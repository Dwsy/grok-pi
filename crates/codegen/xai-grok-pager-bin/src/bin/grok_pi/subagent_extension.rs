use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the bundled Pi child-session lifecycle owner as a standalone
/// extension. The source remains a TypeScript Pi extension; this wrapper only
/// gives the launched Pi process a durable `.ts` path for its lifetime.
pub(super) fn write_subagent_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-subagents-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi subagent extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-subagents/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi subagent extension source")?;
    file.flush().context("flush Pi subagent extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_extension_source_is_a_loadable_typescript_module() {
        let file = write_subagent_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("customType: BRIDGE_TYPE"));
        assert!(source.contains("process.env.PI_GROK_SUBAGENTS !== \"1\""));
        assert!(source.contains("name: \"spawn_subagent\""));
        assert!(source.contains("__pi_grok_subagent_cancel"));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
