use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::Path};
use tempfile::NamedTempFile;

/// Process-private loop extension source + control file for scheduled tasks.
pub(super) struct LoopExtension {
    source: NamedTempFile,
    control: NamedTempFile,
}

impl LoopExtension {
    pub(super) fn source_path(&self) -> &Path {
        self.source.path()
    }

    pub(super) fn control_path(&self) -> &Path {
        self.control.path()
    }
}

/// Materialize the loop extension and empty control file (retained until Pi exits).
pub(super) fn write_loop_extension() -> Result<LoopExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-loop-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi loop extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-loop/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi loop extension source")?;
    source.flush().context("flush Pi loop extension source")?;
    File::open(source.path())
        .and_then(|file| file.sync_all())
        .ok();

    let mut control = tempfile::Builder::new()
        .prefix("pi-grok-loop-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi loop control tempfile")?;
    control
        .write_all(b"{\"tasks\":[]}")
        .context("write Pi loop control seed")?;
    control.flush().context("flush Pi loop control")?;
    File::open(control.path())
        .and_then(|file| file.sync_all())
        .ok();

    Ok(LoopExtension { source, control })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_extension_source_loads() {
        let ext = write_loop_extension().expect("write");
        let source = std::fs::read_to_string(ext.source_path()).expect("read");
        assert!(source.contains("scheduler_create"));
        assert!(source.contains("registerCommand(\"loop\""));
        assert!(source.contains("PI_GROK_LOOP_CONTROL"));
        assert!(source.contains("pi-grok-loop/v1"));
    }
}
