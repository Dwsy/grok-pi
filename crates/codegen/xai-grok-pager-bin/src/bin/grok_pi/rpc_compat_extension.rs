use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the RPC compatibility extension before user extensions load.
///
/// Pi remains in JSONL RPC mode. Runtime monkey-patches only:
/// - optional ExtensionRunner mode rewrite (`rpc` → `tui` when opted in)
/// - capture runner + enrich `get_commands` with extension argument completions
pub(super) fn write_rpc_compat_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-rpc-compat-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi RPC compatibility extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-rpc-compat/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi RPC compatibility extension source")?;
    file.flush()
        .context("flush Pi RPC compatibility extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_compat_extension_monkey_patches_mode_and_get_commands() {
        let file = write_rpc_compat_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("PI_GROK_EXTENSION_TUI_COMPAT"));
        assert!(source.contains("core/extensions/runner.js"));
        assert!(source.contains("setUIContext"));
        assert!(source.contains("mode === \"rpc\" ? \"tui\" : mode"));
        assert!(source.contains("getArgumentCompletions"));
        assert!(source.contains("argumentCompletions"));
        assert!(source.contains("__pi_grok_queue_enqueue__"));
        assert!(source.contains("event.source !== \"extension\""));
        assert!(source.contains("action: \"handled\""));
        assert!(source.contains("core/output-guard.js"));
        // Intercept via process.stdout.write + takeOverStdout — never reassign
        // frozen ESM export writeRawStdout (Node: Cannot redefine property).
        assert!(source.contains("takeOverStdout"));
        assert!(source.contains("process.stdout.write"));
        assert!(!source.contains("module.writeRawStdout"));
        assert!(!source.contains("process.argv ="));
        // Must not edit Pi sources; only host-module runtime hooks.
        assert!(!source.contains("rpc-mode.ts"));
        assert!(!source.contains("modes/rpc/rpc-mode"));
    }
}
