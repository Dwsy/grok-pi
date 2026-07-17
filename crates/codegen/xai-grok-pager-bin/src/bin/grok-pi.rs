//! Native Grok Build TUI backed by the Pi agent core.
//!
//! This binary is intentionally part of `xai-grok-pager-bin`, Grok Build's
//! production TUI composition package. The Pi crate is a protocol adapter only;
//! every terminal surface is created and rendered by `xai-grok-pager`.

use anyhow::{Context, Result};
use clap::Parser;
use pi_grok_adapter::{PiAgent, PiBootstrap, PiRpc, SpawnConfig};
use std::{path::PathBuf, rc::Rc};
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::acp_channels;

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
    "toggle-mouse-reporting",
];

use xai_grok_pager::{
    acp::{AcpConnection, ExternalUiProfile},
    app::{ExternalRunConfig, PagerArgs, run_external},
};

#[derive(Debug, Parser)]
#[command(
    name = "grok-pi",
    version,
    about = "Run the Pi agent core in Grok Build's production TUI"
)]
struct Args {
    /// Pi executable. Use `node` with --pi-prefix-arg for a local Pi build.
    #[arg(long, default_value = "pi")]
    pi_bin: String,

    /// Argument inserted before `--mode rpc` (repeatable).
    #[arg(long = "pi-prefix-arg")]
    pi_prefix_args: Vec<String>,

    /// Working directory for both Pi and the native Grok pager.
    #[arg(long)]
    pi_cwd: Option<PathBuf>,

    /// Use Grok's native inline terminal mode instead of the alternate screen.
    #[arg(long)]
    no_alt_screen: bool,

    /// Start in Grok's native minimal/scrollback renderer.
    #[arg(long, conflicts_with = "fullscreen")]
    minimal: bool,

    /// Start in Grok's native fullscreen renderer.
    #[arg(long, conflicts_with = "minimal")]
    fullscreen: bool,

    /// Print the protocol boundary and exit without starting a terminal.
    #[arg(long)]
    print_capabilities: bool,

    /// Remaining arguments are passed unchanged to Pi after `--mode rpc`.
    #[arg(last = true, allow_hyphen_values = true)]
    pi_args: Vec<String>,
}

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

    let args = Args::parse();
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
    runtime.block_on(LocalSet::new().run_until(run(args)))
}

async fn run(args: Args) -> Result<()> {
    let cwd = match args.pi_cwd {
        Some(path) => std::path::absolute(path).context("failed to resolve --pi-cwd")?,
        None => std::env::current_dir().context("failed to read current directory")?,
    };

    let pi_session_dir = pi_session_dir(&args.pi_args, &cwd);
    let process = PiRpc::spawn(SpawnConfig {
        program: args.pi_bin,
        prefix_args: args.pi_prefix_args,
        cwd: cwd.clone(),
        pi_args: args.pi_args,
    })
    .await?;
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
    let connection = AcpConnection::external(
        client_channel.tx,
        client_channel.rx,
        initial_models,
        initial_commands,
        CancellationToken::new(),
        ExternalUiProfile {
            agent_name: "Pi".to_string(),
            builtin_commands: command_profile.clone(),
        },
    );

    let mut pager_args = PagerArgs::parse_from(["grok-pi"]);
    pager_args.cwd = Some(cwd.clone());
    pager_args.no_alt_screen = args.no_alt_screen;
    pager_args.minimal = args.minimal;
    pager_args.fullscreen = args.fullscreen;
    pager_args.no_auto_update = true;

    run_external(ExternalRunConfig {
        args: pager_args,
        connection,
        session_id,
        session_title,
        session_cwd: Some(cwd),
    })
    .await
}

fn pi_session_dir(pi_args: &[String], cwd: &std::path::Path) -> PathBuf {
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

fn resolve_pi_path(path: &str, cwd: &std::path::Path) -> PathBuf {
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
        assert_eq!(pi_session_dir(&args, &cwd), PathBuf::from("/project/sessions"));
    }
}
