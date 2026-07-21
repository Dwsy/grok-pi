//! `grok-pi` update discovery and install.
//!
//! Read `Dwsy/grok-pi` Releases JSON and install via the published
//! `install.sh` / `install.ps1`. Release discovery prefers the configured JSP
//! proxy to avoid unauthenticated GitHub API rate limits, then falls back to
//! GitHub directly. npm is intentionally not used (unscoped `grok-pi` is a
//! foreign package; scoped `@dwsy/grok-pi` is not published yet).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::auto_update::UpdateAvailable;

/// GitHub Releases "latest" API for this project's published binaries.
pub const PI_GH_RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/Dwsy/grok-pi/releases/latest";
/// JSP proxy route for the GitHub API. Only the proxy prefix is encoded so
/// the upstream host and repository remain visible in the source.
const JSP_PROXY_PREFIX_B64: &str =
    "aHR0cHM6Ly9qc3AuZHdzeS5saW5rL2h0dHAvaHR0cHM6Ly9hcGkuZ2l0aHViLmNvbS9yZXBvcy8=";
const JSP_PROXY_REFERER_PREFIX_B64: &str = "aHR0cHM6Ly9qc3AuZHdzeS5saW5rLz8=";
const JSP_PROXY_REFERER_SUFFIX: &str = "--ver=110&--mode=cors&--type=&--aceh=1&--level=1";

/// Fetch the latest `grok-pi` version string (no leading `v`) from GitHub.
pub async fn fetch_pi_latest_version() -> Result<String> {
    let v = fetch_github_release_latest().await?;
    tracing::info!(%v, source = "github-releases", "pi update: latest version");
    Ok(v)
}

async fn fetch_github_release_latest() -> Result<String> {
    let client = http_client()?;
    let mut errors = Vec::new();
    let proxy_url = format!(
        "{}Dwsy/grok-pi/releases/latest",
        decode_proxy_part(JSP_PROXY_PREFIX_B64)
    );

    for (url, source) in [
        (proxy_url.as_str(), "jsp-proxy"),
        (PI_GH_RELEASES_LATEST_URL, "github-api"),
    ] {
        match fetch_release_from_url(&client, url, source).await {
            Ok(version) => return Ok(version),
            Err(error) => errors.push(format!("{source}: {error}")),
        }
    }

    anyhow::bail!(
        "failed to fetch latest grok-pi release ({})",
        errors.join("; ")
    )
}

async fn fetch_release_from_url(
    client: &reqwest::Client,
    url: &str,
    source: &str,
) -> Result<String> {
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "grok-pi-update-check");
    if source == "jsp-proxy" {
        request = request.header(
            reqwest::header::REFERER,
            format!(
                "{}{}",
                decode_proxy_part(JSP_PROXY_REFERER_PREFIX_B64),
                JSP_PROXY_REFERER_SUFFIX
            ),
        );
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("GET {source} releases/latest"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "{source} releases/latest HTTP {status}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }
    let value: Value = resp
        .json()
        .await
        .with_context(|| format!("decode {source} release JSON"))?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{source} release JSON missing tag_name"))?;
    normalize_version(tag)
}

fn decode_proxy_part(encoded: &str) -> String {
    String::from_utf8(
        BASE64
            .decode(encoded)
            .expect("static proxy URL fragment must decode"),
    )
    .expect("static proxy URL fragment must be UTF-8")
}

fn normalize_version(raw: &str) -> Result<String> {
    let version = raw.trim().trim_start_matches('v').to_string();
    if version.is_empty() {
        anyhow::bail!("empty version string");
    }
    semver::Version::parse(&version).with_context(|| format!("invalid semver '{version}'"))?;
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
pub async fn check_pi_update_background(current: String) -> Option<UpdateAvailable> {
    let latest = match fetch_pi_latest_version().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "pi update: background check failed");
            return None;
        }
    };
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

/// Check and/or install the latest `grok-pi` from GitHub Releases only.
///
/// Returns the installed version when an install ran; `None` for check-only
/// or when already up to date without `--force`.
pub async fn run_pi_update(current: &str, opts: PiUpdateOptions) -> Result<Option<String>> {
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
    install_pi_from_github(&target).await?;
    eprintln!("Installed grok-pi v{target} from GitHub releases.");
    Ok(Some(target))
}

fn print_pi_update_status(current: &str, latest: &str, json: bool) -> Result<()> {
    let update_available = is_remote_newer(latest, current);
    if json {
        let payload = serde_json::json!({
            "current": current,
            "latest": latest,
            "updateAvailable": update_available,
            "sources": ["github-releases"],
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
pub async fn install_pi_update(current: &str, version: Option<&str>) -> Result<String> {
    let installed = run_pi_update(
        current,
        PiUpdateOptions {
            check_only: false,
            force: true,
            version: version.map(str::to_owned),
            json: false,
        },
    )
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
    let script_url = "https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh";
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(format!(
        "curl -fsSL {script_url} | GROK_PI_VERSION={tag} sh"
    ));
    cmd.env("GROK_PI_VERSION", tag);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd.status().await.context("spawn install.sh via curl|sh")?;
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
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd.status().await.context("spawn install.ps1")?;
    if !status.success() {
        anyhow::bail!("install.ps1 exited with {status}");
    }
    Ok(())
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
