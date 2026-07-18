use clap::Parser;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use crate::GROK_PI_VERSION;

#[derive(Debug, Parser)]
#[command(
    name = "grok-pi",
    version = GROK_PI_VERSION,
    about = "Run the Pi agent core in Grok Build's production TUI",
    after_help = "Pi-compatible aliases:\n  -ns  Alias for --no-skills\n  -nc  Alias for --no-context-files\n  -ne  Alias for --no-extensions\n  -nt  Alias for --no-tools\n\nUpdate (GitHub releases only):\n  grok-pi update            Install latest from Dwsy/grok-pi\n  grok-pi update --check    Print current vs latest\n  Welcome Ctrl+U            Same install when an update is offered"
)]
pub(super) struct Args {
    #[command(subcommand)]
    pub(super) command: Option<Command>,

    /// Pi executable. Use `node` with --pi-prefix-arg for a local Pi build.
    #[arg(long, default_value = "pi")]
    pub(super) pi_bin: String,

    /// Argument inserted before `--mode rpc` (repeatable).
    #[arg(long = "pi-prefix-arg")]
    pub(super) pi_prefix_args: Vec<String>,

    /// Working directory for both Pi and the native Grok pager.
    #[arg(long)]
    pub(super) pi_cwd: Option<PathBuf>,

    /// Continue previous session.
    #[arg(short = 'c', long = "continue")]
    pub(super) continue_last_session: bool,

    /// System prompt (default: coding assistant prompt).
    #[arg(long, value_name = "TEXT")]
    pub(super) system_prompt: Option<String>,

    /// Append text or file contents to the system prompt (can be used multiple times).
    #[arg(long = "append-system-prompt", value_name = "TEXT")]
    pub(super) append_system_prompts: Vec<String>,

    /// Disable skills discovery and loading.
    #[arg(long)]
    pub(super) no_skills: bool,

    /// Disable AGENTS.md and CLAUDE.md discovery and loading.
    #[arg(long)]
    pub(super) no_context_files: bool,

    /// Load an extension file (can be used multiple times).
    #[arg(short = 'e', long = "extension", value_name = "PATH")]
    pub(super) extensions: Vec<String>,

    /// Disable extension discovery (explicit -e paths still work).
    #[arg(long)]
    pub(super) no_extensions: bool,

    /// Disable all tools by default (built-in and extension).
    #[arg(long)]
    pub(super) no_tools: bool,

    /// Don't save session (ephemeral).
    #[arg(long)]
    pub(super) no_session: bool,

    /// Set session display name.
    #[arg(short = 'n', long, value_name = "NAME")]
    pub(super) name: Option<String>,

    /// Use Grok's native inline terminal mode instead of the alternate screen.
    #[arg(long)]
    pub(super) no_alt_screen: bool,

    /// Start in Grok's native minimal/scrollback renderer.
    #[arg(long, conflicts_with = "fullscreen")]
    pub(super) minimal: bool,

    /// Start in Grok's native fullscreen renderer.
    #[arg(long, conflicts_with = "minimal")]
    pub(super) fullscreen: bool,

    /// Print the protocol boundary and exit without starting a terminal.
    #[arg(long)]
    pub(super) print_capabilities: bool,

    /// Remaining arguments are passed unchanged to Pi after `--mode rpc`.
    #[arg(last = true, allow_hyphen_values = true)]
    pub(super) pi_args: Vec<String>,
}

#[derive(Debug, clap::Subcommand)]
pub(super) enum Command {
    /// Check for or install grok-pi updates from GitHub Releases only.
    Update {
        /// Only report current vs latest; do not install.
        #[arg(long)]
        check: bool,
        /// Machine-readable status (requires `--check`).
        #[arg(long, requires = "check")]
        json: bool,
        /// Reinstall even when already on the latest version.
        #[arg(long)]
        force: bool,
        /// Install a specific version (e.g. `0.0.2` or `v0.0.2`).
        /// Named `--to` so it does not clash with clap's global `--version`.
        #[arg(long = "to", value_name = "VERSION")]
        version: Option<String>,
    },
}

pub(super) fn normalize_compound_short_flags(
    args: impl IntoIterator<Item = OsString>,
) -> Vec<OsString> {
    let mut parse_options = true;
    args.into_iter()
        .map(|arg| {
            if parse_options && arg.as_os_str() == OsStr::new("--") {
                parse_options = false;
                return arg;
            }
            if !parse_options {
                return arg;
            }
            match arg.to_str() {
                Some("-ns") => OsString::from("--no-skills"),
                Some("-nc") => OsString::from("--no-context-files"),
                Some("-ne") => OsString::from("--no-extensions"),
                Some("-nt") => OsString::from("--no-tools"),
                _ => arg,
            }
        })
        .collect()
}

pub(super) fn pi_args_with_startup_flags(
    mut pi_args: Vec<String>,
    args: &Args,
    bridge_extension: Option<&Path>,
) -> Vec<String> {
    if args.continue_last_session {
        pi_args.push("--continue".to_string());
    }
    if let Some(system_prompt) = &args.system_prompt {
        pi_args.extend(["--system-prompt".to_string(), system_prompt.clone()]);
    }
    for append_system_prompt in &args.append_system_prompts {
        pi_args.extend([
            "--append-system-prompt".to_string(),
            append_system_prompt.clone(),
        ]);
    }
    if args.no_skills {
        pi_args.push("--no-skills".to_string());
    }
    if args.no_context_files {
        pi_args.push("--no-context-files".to_string());
    }
    for extension in &args.extensions {
        pi_args.extend(["--extension".to_string(), extension.clone()]);
    }
    // Explicit --extension paths still load under --no-extensions.
    if let Some(path) = bridge_extension {
        pi_args.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    if args.no_extensions {
        pi_args.push("--no-extensions".to_string());
    }
    if args.no_tools {
        pi_args.push("--no-tools".to_string());
    }
    if args.no_session {
        pi_args.push("--no-session".to_string());
    }
    if let Some(name) = &args.name {
        pi_args.extend(["--name".to_string(), name.clone()]);
    }
    pi_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn continue_flag_is_forwarded_to_pi() {
        let args = Args::try_parse_from(["grok-pi", "--continue"]).unwrap();
        assert!(args.continue_last_session);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            vec!["--continue"]
        );
    }

    #[test]
    fn short_continue_flag_is_forwarded_to_pi() {
        let args = Args::try_parse_from(["grok-pi", "-c"]).unwrap();
        assert!(args.continue_last_session);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            vec!["--continue"]
        );
    }

    #[test]
    fn pi_startup_flags_are_forwarded_to_pi() {
        let args = Args::try_parse_from(normalize_compound_short_flags(
            [
                "grok-pi",
                "--system-prompt",
                "base prompt",
                "--append-system-prompt",
                "first addition",
                "--append-system-prompt",
                "second addition",
                "-ns",
                "-nc",
                "-e",
                "first.ts",
                "--extension",
                "second.ts",
                "-ne",
                "-nt",
                "--no-session",
                "-n",
                "named-session",
            ]
            .into_iter()
            .map(OsString::from),
        ))
        .unwrap();

        assert_eq!(
            pi_args_with_startup_flags(
                args.pi_args.clone(),
                &args,
                Some(Path::new("/tmp/bridge.ts")),
            ),
            vec![
                "--system-prompt",
                "base prompt",
                "--append-system-prompt",
                "first addition",
                "--append-system-prompt",
                "second addition",
                "--no-skills",
                "--no-context-files",
                "--extension",
                "first.ts",
                "--extension",
                "second.ts",
                "--extension",
                "/tmp/bridge.ts",
                "--no-extensions",
                "--no-tools",
                "--no-session",
                "--name",
                "named-session",
            ]
        );
    }

    #[test]
    fn compound_short_flags_after_double_dash_are_not_rewritten() {
        assert_eq!(
            normalize_compound_short_flags(
                ["grok-pi", "--", "-ns", "-nc", "-ne", "-nt"]
                    .into_iter()
                    .map(OsString::from),
            ),
            ["grok-pi", "--", "-ns", "-nc", "-ne", "-nt"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
        );
    }
}
