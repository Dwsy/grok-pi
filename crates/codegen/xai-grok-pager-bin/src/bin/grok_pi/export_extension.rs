use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize default-on Pi session export/share (`/export-html` / `/pi-share`).
///
/// Reuses Pi host export-html + gh gist paths. Grok `/export` stays Markdown.
pub(super) fn write_export_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-export-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi export extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-export/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi export extension source")?;
    file.flush().context("flush Pi export extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_extension_source_is_a_loadable_typescript_module() {
        let file = write_export_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("registerCommand(\"export-html\""));
        assert!(source.contains("registerCommand(\"pi-share\""));
        assert!(source.contains("exportSessionToHtml"));
        assert!(source.contains("getShareViewerUrl"));
        assert!(source.contains("mkdtempSync"));
        assert!(source.contains("path.join(tmpDir, \"session.html\")"));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
