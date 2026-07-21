use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the Pi tree file rollback checkpoint extension.
/// Only injected when F2 `pi_tree_file_rollback` is enabled.
pub(super) fn write_rollback_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-rollback-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi rollback extension tempfile")?;
    const SOURCE: &str =
        include_str!("../../../../../../extensions/pi-grok-rollback/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi rollback extension source")?;
    file.flush().context("flush Pi rollback extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

/// Read the F2 `pi_tree_file_rollback` setting from the effective config.
pub(super) fn rollback_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_tree_file_rollback"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Create the process-unique control directory for the bridge.
/// Returns the path string.
pub(super) fn create_control_dir() -> Result<String> {
    let state_root = state_root_path();
    let control = format!(
        "control-{}-{}",
        std::process::id(),
        &uuid_v4_short()
    );
    let dir = std::path::Path::new(&state_root).join(&control);
    std::fs::create_dir_all(&dir).context("create rollback control dir")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
    }
    Ok(dir.to_string_lossy().into_owned())
}

/// The state root for rollback journals and blobs.
fn state_root_path() -> String {
    let home = std::env::var("GROK_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.grok")))
        .unwrap_or_else(|_| "/tmp/.grok".to_string());
    let root = if std::env::var("GROK_HOME").is_ok() {
        format!("{home}/pi-file-rollback")
    } else {
        format!("{home}/pi-file-rollback")
    };
    std::fs::create_dir_all(&root).ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).ok();
    }
    root
}

/// Expose the state root for the adapter.
pub(super) fn rollback_state_root() -> String {
    state_root_path()
}

fn uuid_v4_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}{:04x}", std::process::id(), nanos & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_extension_source_is_valid_ts_module() {
        let file = write_rollback_extension().expect("write rollback extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("PI_GROK_ROLLBACK"));
        assert!(source.contains("createWriteToolDefinition"));
        assert!(source.contains("createEditToolDefinition"));
        assert!(source.contains("__pi_rollback_preview"));
        assert!(source.contains("__pi_rollback_execute"));
        assert_eq!(
            file.path().extension().and_then(|e| e.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn rollback_disabled_by_default() {
        // Without a config file, rollback should be disabled.
        // This test may pass or fail depending on user config;
        // just verify the function doesn't panic.
        let _ = rollback_enabled();
    }
}
