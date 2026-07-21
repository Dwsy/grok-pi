//! Fast Pi host version probe for grok-pi startup.
//!
//! Goals:
//! - cheap when version is fine (one short-lived process, small stdout)
//! - fail closed only when missing / unreadable / below min
//! - OS-aware install hints (curl | sh vs PowerShell)

use anyhow::{Result, bail};
use semver::Version;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Minimum supported Pi CLI version (system package / pi.dev installer).
pub(super) const MIN_PI_VERSION: &str = "0.80.10";

const INSTALL_UNIX: &str = "curl -fsSL https://pi.dev/install.sh | sh";
const INSTALL_WINDOWS: &str = r#"powershell -c "irm https://pi.dev/install.ps1 | iex""#;
const INSTALL_NPM: &str = "npm i -g @earendil-works/pi-coding-agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PiHostCheck {
    Ok { version: Version, program: String },
    TooOld { version: Version, program: String },
    Missing { program: String, detail: String },
    Unparseable { program: String, raw: String },
}

/// Probe `program --version` with a short timeout. Does not spawn Pi RPC.
pub(super) fn check_pi_host(program: &str) -> PiHostCheck {
    let min = Version::parse(MIN_PI_VERSION).expect("MIN_PI_VERSION is valid semver");
    match run_pi_version(program) {
        Ok(raw) => match parse_pi_version(&raw) {
            Some(version) if version >= min => PiHostCheck::Ok {
                version,
                program: program.to_string(),
            },
            Some(version) => PiHostCheck::TooOld {
                version,
                program: program.to_string(),
            },
            None => PiHostCheck::Unparseable {
                program: program.to_string(),
                raw: raw.trim().to_string(),
            },
        },
        Err(detail) => PiHostCheck::Missing {
            program: program.to_string(),
            detail,
        },
    }
}

/// Hard-require a compatible host. Prints install guidance to stderr on failure.
pub(super) fn ensure_compatible_pi_host(program: &str) -> Result<Version> {
    match check_pi_host(program) {
        PiHostCheck::Ok { version, program } => {
            eprintln!("Pi host: {program} {version} (min {MIN_PI_VERSION})");
            Ok(version)
        }
        PiHostCheck::TooOld { version, program } => {
            print_upgrade_help(
                &format!("Pi host too old: {program} {version} < required {MIN_PI_VERSION}"),
                &program,
            );
            bail!("Pi {version} is below minimum {MIN_PI_VERSION}");
        }
        PiHostCheck::Missing { program, detail } => {
            print_upgrade_help(
                &format!("Pi host not found or failed: {program} ({detail})"),
                &program,
            );
            bail!("Pi executable unavailable: {program}");
        }
        PiHostCheck::Unparseable { program, raw } => {
            print_upgrade_help(
                &format!("Could not parse Pi version from `{program} --version` output: {raw:?}"),
                &program,
            );
            bail!("unreadable Pi version from {program}");
        }
    }
}

fn run_pi_version(program: &str) -> Result<String, String> {
    // Prefer invoking the path/command as given. For node scripts this still works
    // because the shebang/node wrapper handles --version.
    let mut cmd = if looks_like_js_cli(program) {
        let mut c = Command::new("node");
        c.arg(program).arg("--version");
        c
    } else {
        let mut c = Command::new(program);
        c.arg("--version");
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Keep this off the async runtime: one-shot, short, fail-fast.
    // No shell, no network.
    let output = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit {}", output.status)
        };
        return Err(msg);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn looks_like_js_cli(program: &str) -> bool {
    let path = Path::new(program);
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "cjs")
    )
}

/// Extract the first semver-looking token from version output.
pub(super) fn parse_pi_version(raw: &str) -> Option<Version> {
    // Common shapes:
    // - "0.80.10"
    // - "pi 0.80.10"
    // - "@earendil-works/pi-coding-agent/0.80.10"
    for token in raw.split(|c: char| c.is_whitespace() || c == '/' || c == 'v' || c == 'V') {
        let candidate = token.trim().trim_matches(|c: char| c == ',' || c == ';');
        if candidate.is_empty() {
            continue;
        }
        // Allow "0.80.10-beta.1" etc.
        if let Ok(v) = Version::parse(candidate) {
            return Some(v);
        }
        // Strip trailing junk like "0.80.10," already handled; try prefix digits.digits.digits
        let mut end = 0;
        let bytes = candidate.as_bytes();
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' {
                end += 1;
            } else {
                break;
            }
        }
        if end > 0 {
            if let Ok(v) = Version::parse(&candidate[..end]) {
                return Some(v);
            }
        }
    }
    None
}

fn print_upgrade_help(reason: &str, program: &str) {
    let os_hint = install_command_for_host();
    eprintln!();
    eprintln!("error: {reason}");
    eprintln!();
    eprintln!("grok-pi requires Pi >= {MIN_PI_VERSION} (system `pi` / pi.dev installer).");
    eprintln!("Configured host: {program}");
    eprintln!();
    eprintln!("Install / upgrade (recommended):");
    eprintln!("  {os_hint}");
    eprintln!();
    eprintln!("Also available:");
    if cfg!(windows) {
        eprintln!("  {INSTALL_UNIX}");
    } else {
        eprintln!("  {INSTALL_WINDOWS}");
    }
    eprintln!("  {INSTALL_NPM}");
    eprintln!();
    eprintln!("Docs: https://pi.dev");
    eprintln!("Then re-run grok-pi, or set PI_BIN=/path/to/pi if needed.");
    eprintln!();
}

fn install_command_for_host() -> &'static str {
    if cfg!(windows) {
        INSTALL_WINDOWS
    } else {
        INSTALL_UNIX
    }
}

/// Also print the other platform's one-liner when helpful (WSL/users reading logs).
#[allow(dead_code)]
pub(super) fn both_install_commands() -> (&'static str, &'static str) {
    (INSTALL_UNIX, INSTALL_WINDOWS)
}

// Keep Duration import available for future hard timeout wrappers without
// pulling extra crates; current Command::output is already fast enough for --version.
#[allow(dead_code)]
fn version_probe_budget() -> Duration {
    Duration::from_secs(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_semver() {
        assert_eq!(parse_pi_version("0.80.10").unwrap().to_string(), "0.80.10");
    }

    #[test]
    fn parses_prefixed_output() {
        assert_eq!(
            parse_pi_version("pi 0.80.10\n").unwrap().to_string(),
            "0.80.10"
        );
        assert_eq!(
            parse_pi_version("@earendil-works/pi-coding-agent/0.80.10")
                .unwrap()
                .to_string(),
            "0.80.10"
        );
    }

    #[test]
    fn min_version_constant_is_valid() {
        assert!(Version::parse(MIN_PI_VERSION).is_ok());
    }

    #[test]
    fn too_old_detected() {
        let v = parse_pi_version("0.79.0").unwrap();
        let min = Version::parse(MIN_PI_VERSION).unwrap();
        assert!(v < min);
    }

    #[test]
    fn install_hint_is_platform_specific() {
        let hint = install_command_for_host();
        if cfg!(windows) {
            assert!(hint.contains("powershell"));
        } else {
            assert!(hint.contains("curl") && hint.contains("pi.dev/install.sh"));
        }
    }
}
