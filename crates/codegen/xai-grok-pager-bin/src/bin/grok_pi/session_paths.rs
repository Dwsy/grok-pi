use std::path::{Path, PathBuf};

pub(super) fn pi_session_dir(pi_args: &[String], cwd: &Path) -> PathBuf {
    let configured = pi_args
        .windows(2)
        .filter(|args| args[0] == "--session-dir")
        .map(|args| args[1].as_str())
        .next_back()
        .map(|path| resolve_pi_path(path, cwd))
        .or_else(|| {
            std::env::var("PI_CODING_AGENT_SESSION_DIR")
                .ok()
                .filter(|path| !path.trim().is_empty())
                .map(|path| resolve_pi_path(&path, cwd))
        });
    configured.unwrap_or_else(|| {
        let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pi/agent")))
            .unwrap_or_else(|| PathBuf::from(".pi/agent"));
        resolve_pi_path(&agent_dir.to_string_lossy(), cwd).join("sessions")
    })
}

fn resolve_pi_path(path: &str, cwd: &Path) -> PathBuf {
    let path = path.trim();
    let expanded = path
        .strip_prefix("~/")
        .and_then(|suffix| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)))
        .unwrap_or_else(|| PathBuf::from(path));
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_dir_uses_the_last_pi_session_dir_argument() {
        let cwd = PathBuf::from("/project");
        let args = vec![
            "--session-dir".to_string(),
            "old".to_string(),
            "--session-dir".to_string(),
            "sessions".to_string(),
        ];
        assert_eq!(
            pi_session_dir(&args, &cwd),
            PathBuf::from("/project/sessions")
        );
    }
}
