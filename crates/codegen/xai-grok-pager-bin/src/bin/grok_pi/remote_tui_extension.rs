use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the experimental Remote TUI probe as a standalone extension.
/// Only loaded when PI_GROK_REMOTE_TUI=1.
pub(super) fn write_remote_tui_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-remote-tui-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi remote-tui extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-remote-tui/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi remote-tui extension source")?;
    file.flush()
        .context("flush Pi remote-tui extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_tui_extension_source_is_a_loadable_typescript_module() {
        let file = write_remote_tui_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("name: \"remote-tui\"") || source.contains("\"remote-tui\""));
        assert!(source.contains("PI_GROK_REMOTE_TUI"));
        assert!(source.contains("RemoteTuiProbeList"));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
