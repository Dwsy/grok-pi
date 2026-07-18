use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Inject a headless Pi extension that generates display-only session recaps
/// via `complete()` and emits `pi-grok-recap/v1` custom messages for the adapter.
pub(super) fn write_recap_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-recap-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi recap extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-recap/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi recap extension source")?;
    file.flush().context("flush Pi recap extension source")?;
    File::open(file.path()).and_then(|f| f.sync_all()).ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recap_extension_source_is_a_loadable_typescript_module() {
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("__pi_grok_recap"));
        assert!(source.contains("registerCommand(COMMAND"));
        assert!(source.contains("pi-grok-recap/v1"));
        assert!(source.contains("from \"@earendil-works/pi-ai/compat\""));
        assert!(file.path().extension().and_then(|e| e.to_str()) == Some("ts"));
    }
}
