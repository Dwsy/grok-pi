use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize default-on Pi auth commands (`/login` / `/logout`).
///
/// Requires system Pi >= 0.80.10 (`modelRuntime.login` + Remote TUI).
/// Bare names: Grok external profile does not reserve login/logout.
pub(super) fn write_auth_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-auth-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi auth extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-auth/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi auth extension source")?;
    file.flush().context("flush Pi auth extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_extension_source_is_a_loadable_typescript_module() {
        let file = write_auth_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("registerCommand(\"login\""));
        assert!(source.contains("registerCommand(\"logout\""));
        assert!(source.contains("modelRuntime") || source.contains("resolveRuntime"));
        assert!(source.contains("OAuthSelectorComponent"));
        assert!(source.contains("LoginDialogComponent"));
        assert!(source.contains("PI_GROK_REMOTE_TUI"));
        // Nested openCustom would tear down LoginDialog during Grok CLI method select.
        assert!(source.contains("showOverlay"));
        assert!(source.contains("showAuthPrompt"));
        assert!(source.contains("prompt.signal"));
        assert!(source.contains("value: success"));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
