//! Native Grok Build TUI backed by the Pi agent core.
//!
//! This binary is intentionally part of `xai-grok-pager-bin`, Grok Build's
//! production TUI composition package. The Pi crate is a protocol adapter only;
//! every terminal surface is created and rendered by `xai-grok-pager`.

#[path = "grok_pi/bash_extension.rs"]
mod bash_extension;
#[path = "grok_pi/cli.rs"]
mod cli;
#[path = "grok_pi/recap_extension.rs"]
mod recap_extension;
#[path = "grok_pi/remote_tui_extension.rs"]
mod remote_tui_extension;
#[path = "grok_pi/session_paths.rs"]
mod session_paths;
#[path = "grok_pi/subagent_extension.rs"]
mod subagent_extension;
#[path = "grok_pi/tree_bridge.rs"]
mod tree_bridge;

use anyhow::{Context, Result};
use clap::Parser;
use pi_grok_adapter::{PiAgent, PiBootstrap, PiRpc, SpawnConfig};
use std::rc::Rc;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::acp_channels;
use xai_grok_pager::{
    acp::{AcpConnection, ExternalLogoArt, ExternalUiProfile},
    app::{ExternalRunConfig, PagerArgs, run_external},
};

use bash_extension::write_bash_extension;
use cli::{Args, Command, normalize_compound_short_flags, pi_args_with_startup_flags};
use recap_extension::write_recap_extension;
use remote_tui_extension::write_remote_tui_extension;
use session_paths::pi_session_dir;
use subagent_extension::write_subagent_extension;
use tree_bridge::write_navigate_tree_extension;

/// Grok pager commands that are meaningful when Pi is the ACP backend.
///
/// This is a composition policy, not an adapter feature. The commands below
/// are implemented by the production Grok pager or translated through its ACP
/// actions. Pi-advertised extension commands are merged dynamically.
const PI_GROK_NATIVE_COMMANDS: &[&str] = &[
    // Process and command discovery.
    "exit",
    "help",
    // ACP operations with an explicit Pi implementation.
    "new",
    "compact",
    "model",
    "effort",
    "rename",
    "resume",
    // Pi session entry tree via native ArgPicker + adapter navigate.
    "tree",
    // Native multi-session overview; idle rows come from pi/session/list.
    "dashboard",
    // Display-only session recap via injected Pi extension + adapter bridge.
    "recap",
    // Native Grok transcript/navigation surfaces over the Pi-backed session.
    "copy",
    "find",
    "transcript",
    "export",
    "expand",
    "queue",
    // Native Grok terminal/composer appearance controls.
    "multiline",
    "compact-mode",
    "vim-mode",
    "theme",
    "timestamps",
    "timeline",
    "toggle-mouse-reporting",
    // Pager-native Pi resource manager (`/pi-config`, `/pi-resources`).
    "pi-config",
];

/// Block-character π mark for the native Grok welcome / minimal logo surface.
/// Matches Pi's static logo (`print_static_logo`): two-space indent + block art.
/// Kept as plain full-block art so it remains legible on terminals that cannot
/// render Grok's default braille logo. The welcome logo renderer pads rows to a
/// common visual width so per-line centering does not drift the glyph.
const PI_LOGO: &str = "\
  ██████\n\
  ██  ██\n\
  ████  ██\n\
  ██    ██\n\
";

/// Product version for `grok-pi --version` (release tag / git describe).
/// Not the upstream workspace crate version (`0.1.220-alpha.*`).
const GROK_PI_VERSION: &str = env!("GROK_PI_VERSION");

fn main() -> Result<()> {
    // Keep the exact production pager process hooks. In particular, Mermaid
    // rendering re-enters this binary with an internal worker argument and
    // therefore must be handled before clap parses the public `grok-pi` CLI.
    xai_grok_pager_minimal::install();
    if let Some(code) = xai_grok_pager::app::mermaid_worker::maybe_run_render_subprocess() {
        std::process::exit(code);
    }
    xai_crash_handler::install_terminal_restore_only();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = Args::parse_from(normalize_compound_short_flags(std::env::args_os()));
    if args.print_capabilities {
        println!(
            "{}",
            include_str!("../../../pi-grok-adapter/docs/capabilities.json")
        );
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the Grok pager Tokio runtime")?;
    if let Some(Command::Update {
        check,
        json,
        force,
        version,
    }) = args.command
    {
        return runtime.block_on(async move {
            xai_grok_update::run_pi_update(xai_grok_update::PiUpdateOptions {
                check_only: check,
                force,
                version,
                json,
            })
            .await?;
            Ok(())
        });
    }
    runtime.block_on(LocalSet::new().run_until(run(args)))
}

async fn run(mut args: Args) -> Result<()> {
    let cwd = match args.pi_cwd.as_ref() {
        Some(path) => std::path::absolute(path).context("failed to resolve --pi-cwd")?,
        None => std::env::current_dir().context("failed to read current directory")?,
    };

    // Discover Pi theme JSON (embedded dark/light + ~/.pi/agent/themes + .pi/themes)
    // so `/theme` can list and apply them as `pi:<name>`.
    let _theme_report = xai_grok_pager::theme::pi::init_discovery(&cwd);

    let pi_session_dir = pi_session_dir(&args.pi_args, &cwd);
    // Keep the bridge extension alive for the process lifetime. Pi loads it via
    // `--extension`; drop would unlink the temp path before Pi finishes boot.
    let navigate_tree_extension = write_navigate_tree_extension()
        .context("failed to create Pi navigateTree bridge extension")?;
    // Default ON. Disable with PI_GROK_BASH=0 (or false/off/no).
    // Note: re-registers tool "bash" and can conflict with user packages
    // such as pi-tool-display — set PI_GROK_BASH=0 if bootstrap fails on tool clash.
    let bash_enabled = env_flag_default_on("PI_GROK_BASH");
    let bash_extension = if bash_enabled {
        Some(write_bash_extension().context("failed to create Pi Bash extension")?)
    } else {
        None
    };
    let subagent_extension =
        write_subagent_extension().context("failed to create Pi subagent extension")?;
    let recap_extension = write_recap_extension().context("failed to create Pi recap extension")?;
    // Experimental Remote TUI — default ON. Disable with PI_GROK_REMOTE_TUI=0.
    let remote_tui_enabled = env_flag_default_on("PI_GROK_REMOTE_TUI");
    let remote_tui_extension = if remote_tui_enabled {
        Some(write_remote_tui_extension().context("failed to create Pi remote-tui extension")?)
    } else {
        None
    };
    let mut pi_args = pi_args_with_startup_flags(
        std::mem::take(&mut args.pi_args),
        &args,
        Some(navigate_tree_extension.path()),
    );
    // Explicit extensions remain active under --no-extensions. The Pi
    // subagent/recap runtimes are lifecycle-only and never own a terminal surface.
    pi_args.extend([
        "--extension".to_string(),
        subagent_extension.path().to_string_lossy().into_owned(),
        "--extension".to_string(),
        recap_extension.path().to_string_lossy().into_owned(),
    ]);
    if let Some(path) = bash_extension.as_ref().map(|extension| extension.source_path()) {
        pi_args.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(path) = remote_tui_extension.as_ref().map(|file| file.path()) {
        pi_args.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    // Identifies this Pi child as running under the grok-pi host for user extensions.
    let mut env = vec![
        ("PI_GROK".to_string(), "1".to_string()),
        ("PI_GROK_SUBAGENTS".to_string(), "1".to_string()),
    ];
    if let Some(extension) = bash_extension.as_ref() {
        env.push(("PI_GROK_BASH".to_string(), "1".to_string()));
        env.push((
            "PI_GROK_BASH_CONTROL_META".to_string(),
            extension.control_meta_path().to_string_lossy().into_owned(),
        ));
    }
    if remote_tui_enabled {
        // Extension host gates on this exact value.
        env.push(("PI_GROK_REMOTE_TUI".to_string(), "1".to_string()));
        // Pi RPC child has no real TTY; pass host size so Remote TUI is full-width
        // like interactive Pi (not a fixed 72-col probe box).
        if let Some((cols, rows)) = host_terminal_size() {
            env.push(("COLUMNS".to_string(), cols.to_string()));
            env.push(("LINES".to_string(), rows.to_string()));
            env.push(("PI_GROK_REMOTE_TUI_WIDTH".to_string(), cols.to_string()));
            env.push(("PI_GROK_REMOTE_TUI_ROWS".to_string(), rows.to_string()));
        }
    }
    let process = PiRpc::spawn(SpawnConfig {
        program: args.pi_bin,
        prefix_args: args.pi_prefix_args,
        cwd: cwd.clone(),
        pi_args,
        env,
    })
    .await?;
    let bash_control_meta = bash_extension
        .as_ref()
        .map(|extension| extension.control_meta_path().to_path_buf());
    // Hold the NamedTempFiles so the extension paths remain valid.
    let _navigate_tree_extension = navigate_tree_extension;
    let _bash_extension = bash_extension;
    let _subagent_extension = subagent_extension;
    let _recap_extension = recap_extension;
    let _remote_tui_extension = remote_tui_extension;
    let bootstrap = PiBootstrap::load(&process.rpc)
        .await
        .context("failed to bootstrap Pi RPC state")?;

    let initial_models = bootstrap.acp_models();
    let initial_commands = bootstrap.acp_commands();
    let session_id = bootstrap.session_id().to_string();
    let session_title = bootstrap
        .session_title()
        .map(str::to_owned)
        .or_else(|| Some("Pi".to_string()));

    let (client_channel, mut agent_channel) = acp_channels();
    let adapter = Rc::new(PiAgent::new(
        process.rpc,
        agent_channel.tx.clone(),
        bootstrap,
        pi_session_dir,
        bash_control_meta,
    ));

    let event_adapter = adapter.clone();
    tokio::task::spawn_local(async move {
        event_adapter.run_events(process.events).await;
    });

    let route_adapter = adapter.clone();
    tokio::task::spawn_local(async move {
        while let Some(message) = agent_channel.rx.recv().await {
            message.route_to_agent(route_adapter.clone(), |future| {
                tokio::task::spawn_local(future);
            });
        }
    });

    let command_profile = PI_GROK_NATIVE_COMMANDS
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    // External ACP skips shell `initialize`, so recap must be enabled here.
    // Adapter still implements initialize.meta.sessionRecap for non-external
    // paths; `/recap` stays hidden until this flag is true.
    let mut connection = AcpConnection::external(
        client_channel.tx,
        client_channel.rx,
        initial_models,
        initial_commands,
        CancellationToken::new(),
        ExternalUiProfile {
            agent_name: "Pi".to_string(),
            builtin_commands: command_profile.clone(),
            logo: Some(ExternalLogoArt {
                full: PI_LOGO,
                small: PI_LOGO,
            }),
            // Grok worktree product flow is not wired for Pi yet.
            hide_new_worktree: true,
            changelog_url: Some("https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD"),
        },
    );
    connection.session_recap_available = true;

    let mut pager_args = PagerArgs::parse_from(["grok-pi"]);
    pager_args.cwd = Some(cwd.clone());
    pager_args.no_alt_screen = args.no_alt_screen;
    pager_args.minimal = args.minimal;
    pager_args.fullscreen = args.fullscreen;
    // Enable the Pi-specific update check (GitHub Releases only).
    // Set GROK_PI_NO_AUTO_UPDATE=1 to disable the background check.
    pager_args.no_auto_update = std::env::var_os("GROK_PI_NO_AUTO_UPDATE").is_some();

    run_external(ExternalRunConfig {
        args: pager_args,
        connection,
        session_id,
        session_title,
        session_cwd: Some(cwd),
        // Stock Grok lands on Welcome with logo unless --continue/--resume.
        // Only `-c/--continue` skips Welcome and attaches the Pi session now.
        resume_existing_session: args.continue_last_session,
    })
    .await
}

/// Best-effort host terminal size for Remote TUI viewport (Pi child has no TTY).
fn host_terminal_size() -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        // SAFETY: ioctl(TIOCGWINSZ) on stdout; fails cleanly when not a TTY.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
                && ws.ws_col > 0
                && ws.ws_row > 0
            {
                return Some((ws.ws_col, ws.ws_row));
            }
        }
    }
    None
}

/// Feature flags that default to ON. Explicit `0`/`false`/`off`/`no` disables.
/// Unset or any other value (including `1`) enables.
fn env_flag_default_on(name: &str) -> bool {
    match std::env::var(name) {
        Err(_) => true,
        Ok(value) => {
            let v = value.trim();
            !(v.eq_ignore_ascii_case("0")
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        }
    }
}

#[cfg(test)]
mod env_flag_tests {
    use super::env_flag_default_on;

    #[test]
    fn default_on_when_unset() {
        // SAFETY: test-only env mutation in this unit test process.
        unsafe {
            std::env::remove_var("PI_GROK_TEST_FLAG_DEFAULT_ON");
        }
        assert!(env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"));
    }

    #[test]
    fn off_values_disable() {
        for value in ["0", "false", "OFF", "No"] {
            unsafe {
                std::env::set_var("PI_GROK_TEST_FLAG_DEFAULT_ON", value);
            }
            assert!(!env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"), "{value}");
        }
        unsafe {
            std::env::set_var("PI_GROK_TEST_FLAG_DEFAULT_ON", "1");
        }
        assert!(env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"));
        unsafe {
            std::env::remove_var("PI_GROK_TEST_FLAG_DEFAULT_ON");
        }
    }
}
