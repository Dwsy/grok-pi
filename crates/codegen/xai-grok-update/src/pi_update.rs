//! `grok-pi` update discovery.
//!
//! Source order (first success wins):
//! 1. GitHub Releases JSON for `Dwsy/grok-pi`
//! 2. npm registry JSON via npmmirror, then registry.npmjs.org
//!
//! Intentionally independent of stock Grok installers (`@xai-official/grok`,
//! x.ai CLI channel pointers).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::auto_update::UpdateAvailable;
use crate::version::get_installed_grok_version;

/// GitHub Releases "latest" API for this project's published binaries.
pub const PI_GH_RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/Dwsy/grok-pi/releases/latest";

/// npm package name used as the secondary version source.
pub const PI_NPM_PACKAGE: &str = "grok-pi";

/// Prefer Chinese mirror, then the public npm registry.
pub const PI_NPM_LATEST_URLS: &[&str] = &[
    "https://registry.npmmirror.com/grok-pi/latest",
    "https://registry.npmjs.org/grok-pi/latest",
];

/// Fetch the latest `grok-pi` version string (no leading `v`).
pub async fn fetch_pi_latest_version() -> Result<String> {
    match fetch_github_release_latest().await {
        Ok(v) => {
            tracing::info!(%v, source = "github-releases", "pi update: latest version");
            Ok(v)
        }
        Err(gh_err) => {
            tracing::warn!(
                error = %gh_err,
                "pi update: GitHub releases unavailable; trying npm registry mirrors"
            );
            match fetch_npm_mirror_latest().await {
                Ok(v) => {
                    tracing::info!(%v, source = "npm-mirror", "pi update: latest version");
                    Ok(v)
                }
                Err(npm_err) => Err(anyhow!(
                    "pi update check failed: github={gh_err:#}; npm={npm_err:#}"
                )),
            }
        }
    }
}

async fn fetch_github_release_latest() -> Result<String> {
    let client = http_client()?;
    let resp = client
        .get(PI_GH_RELEASES_LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "grok-pi-update-check")
        .send()
        .await
        .context("GET GitHub releases/latest")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "GitHub releases/latest HTTP {status}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }
    let value: Value = resp.json().await.context("decode GitHub release JSON")?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("GitHub release JSON missing tag_name"))?;
    normalize_version(tag)
}

async fn fetch_npm_mirror_latest() -> Result<String> {
    let client = http_client()?;
    let mut last_err = None;
    for url in PI_NPM_LATEST_URLS {
        match fetch_npm_latest_from(&client, url).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(%url, error = %e, "pi update: npm mirror failed");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no npm mirror URLs configured")))
}

async fn fetch_npm_latest_from(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", "grok-pi-update-check")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "npm latest HTTP {status} for {url}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }
    let value: Value = resp.json().await.context("decode npm latest JSON")?;
    // registry.npmjs.org / npmmirror `.../latest` returns the version document
    // with a top-level `version` field.
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("npm latest JSON missing version"))?;
    normalize_version(version)
}

fn normalize_version(raw: &str) -> Result<String> {
    let version = raw.trim().trim_start_matches('v').to_string();
    if version.is_empty() {
        anyhow::bail!("empty version string");
    }
    semver::Version::parse(&version)
        .with_context(|| format!("invalid semver '{version}'"))?;
    Ok(version)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .context("build HTTP client")
}

/// Background check: `Some(UpdateAvailable)` when remote is newer than the
/// running binary; `None` when current or on any hard failure.
pub async fn check_pi_update_background() -> Option<UpdateAvailable> {
    let latest = match fetch_pi_latest_version().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "pi update: background check failed");
            return None;
        }
    };
    let current = get_installed_grok_version();
    if is_remote_newer(&latest, &current) {
        Some(UpdateAvailable {
            latest_version: latest,
        })
    } else {
        None
    }
}

fn is_remote_newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(remote), Ok(local)) => remote > local,
        _ => {
            tracing::warn!(%current, %latest, "pi update: semver parse failed");
            false
        }
    }
}

/// Options for [`run_pi_update`].
#[derive(Debug, Clone, Default)]
pub struct PiUpdateOptions {
    /// Only print status; do not install.
    pub check_only: bool,
    /// Install even when the remote version is not newer.
    pub force: bool,
    /// Pin a specific semver (with or without `v` prefix). `None` = latest.
    pub version: Option<String>,
    /// Emit machine-readable JSON for `--check`.
    pub json: bool,
}

/// Check and/or install the latest `grok-pi`.
///
/// Install order:
/// 1. GitHub release asset via published `install.sh` / `install.ps1`
/// 2. `npm install -g grok-pi@…` via npmmirror, then registry.npmjs.org
///
/// Returns the installed version when an install ran; `None` for check-only
/// or when already up to date without `--force`.
pub async fn run_pi_update(opts: PiUpdateOptions) -> Result<Option<String>> {
    let current = get_installed_grok_version();
    let target = match opts.version.as_deref() {
        Some(v) => normalize_version(v)?,
        None => fetch_pi_latest_version().await?,
    };

    if opts.check_only {
        print_pi_update_status(&current, &target, opts.json)?;
        return Ok(None);
    }

    if !opts.force && !is_remote_newer(&target, &current) {
        eprintln!("Already up to date (v{current}).");
        return Ok(None);
    }

    eprintln!("Updating grok-pi {current} → {target}…");
    match install_pi_from_github(&target).await {
        Ok(()) => {
            eprintln!("Installed grok-pi v{target} from GitHub releases.");
            Ok(Some(target))
        }
        Err(gh_err) => {
            tracing::warn!(error = %gh_err, "pi update: GitHub install failed; trying npm");
            eprintln!("GitHub install failed ({gh_err:#}); trying npm…");
            install_pi_from_npm(&target).await?;
            eprintln!("Installed grok-pi v{target} from npm.");
            Ok(Some(target))
        }
    }
}

fn print_pi_update_status(current: &str, latest: &str, json: bool) -> Result<()> {
    let update_available = is_remote_newer(latest, current);
    if json {
        let payload = serde_json::json!({
            "current": current,
            "latest": latest,
            "updateAvailable": update_available,
            "sources": ["github-releases", "npm-mirror"],
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("Current:  v{current}");
    println!("Latest:   v{latest}");
    if update_available {
        println!("Update available. Run: grok-pi update");
        println!("Or press Ctrl+U on the Welcome screen when prompted.");
    } else {
        println!("Already up to date.");
    }
    Ok(())
}

/// Install a specific version (or latest when `version` is `None`).
/// Used by Welcome **Ctrl+U** after quit-for-update — always installs
/// (force) because the UI already decided an update is desired.
pub async fn install_pi_update(version: Option<&str>) -> Result<String> {
    let installed = run_pi_update(PiUpdateOptions {
        check_only: false,
        force: true,
        version: version.map(str::to_owned),
        json: false,
    })
    .await?;
    installed.ok_or_else(|| anyhow!("install produced no version"))
}

async fn install_pi_from_github(version: &str) -> Result<()> {
    let tag = format!("v{}", version.trim_start_matches('v'));
    #[cfg(windows)]
    {
        install_pi_windows_ps1(&tag).await
    }
    #[cfg(not(windows))]
    {
        install_pi_unix_sh(&tag).await
    }
}

#[cfg(not(windows))]
async fn install_pi_unix_sh(tag: &str) -> Result<()> {
    // The installer script is identical across tags; pin the binary via env.
    let script_url =
        "https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh";
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(format!(
        "curl -fsSL {script_url} | GROK_PI_VERSION={tag} sh"
    ));
    cmd.env("GROK_PI_VERSION", tag);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd
        .status()
        .await
        .context("spawn install.sh via curl|sh")?;
    if !status.success() {
        anyhow::bail!("install.sh exited with {status}");
    }
    Ok(())
}

#[cfg(windows)]
async fn install_pi_windows_ps1(tag: &str) -> Result<()> {
    let script = format!(
        "$env:GROK_PI_VERSION='{tag}'; irm https://github.com/Dwsy/grok-pi/releases/latest/download/install.ps1 | iex"
    );
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd.status().await.context("spawn install.ps1")?;
    if !status.success() {
        anyhow::bail!("install.ps1 exited with {status}");
    }
    Ok(())
}

async fn install_pi_from_npm(version: &str) -> Result<()> {
    let pkg = format!("{PI_NPM_PACKAGE}@{version}");
    let registries = [
        "https://registry.npmmirror.com",
        "https://registry.npmjs.org",
    ];
    let mut last_err = None;
    for registry in registries {
        let mut cmd = tokio::process::Command::new("npm");
        cmd.args(["install", "--global", &pkg, &format!("--registry={registry}")]);
        cmd.stdin(std::process::Stdio::null());
        xai_grok_tools::util::detach_command(&mut cmd);
        match cmd.status().await {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                last_err = Some(anyhow!("npm install failed via {registry}: {status}"));
            }
            Err(e) => {
                last_err = Some(anyhow!("spawn npm via {registry}: {e}"));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("npm install failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_version("v0.0.2").unwrap(), "0.0.2");
        assert_eq!(normalize_version("0.0.2").unwrap(), "0.0.2");
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_version("").is_err());
        assert!(normalize_version("latest").is_err());
    }

    #[test]
    fn remote_newer_compares_semver() {
        assert!(is_remote_newer("0.0.2", "0.0.1"));
        assert!(!is_remote_newer("0.0.1", "0.0.2"));
        assert!(!is_remote_newer("0.0.2", "0.0.2"));
    }
}
