use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::Path};
use tempfile::{NamedTempFile, TempDir};

/// Process-private ask_user_question extension + response directory.
pub(super) struct AskUserExtension {
    source: NamedTempFile,
    dir: TempDir,
}

impl AskUserExtension {
    pub(super) fn source_path(&self) -> &Path {
        self.source.path()
    }

    pub(super) fn dir_path(&self) -> &Path {
        self.dir.path()
    }
}

/// Materialize the Q&A extension and empty control directory (retained until Pi exits).
pub(super) fn write_ask_user_extension() -> Result<AskUserExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-ask-user-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi ask_user_question extension tempfile")?;
    const SOURCE: &str =
        include_str!("../../../../../../extensions/pi-grok-ask-user-question/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi ask_user_question extension source")?;
    source
        .flush()
        .context("flush Pi ask_user_question extension source")?;
    File::open(source.path())
        .and_then(|file| file.sync_all())
        .ok();

    let dir = tempfile::Builder::new()
        .prefix("pi-grok-ask-user-dir-")
        .tempdir()
        .context("create Pi ask_user_question control directory")?;

    Ok(AskUserExtension { source, dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_user_extension_source_loads() {
        let ext = write_ask_user_extension().expect("write");
        let source = std::fs::read_to_string(ext.source_path()).expect("read");
        assert!(source.contains("ask_user_question"));
        assert!(source.contains("PI_GROK_ASK_USER_DIR"));
        assert!(source.contains("right questions to nail the details"));
        assert!(ext.dir_path().is_dir());
    }
}
