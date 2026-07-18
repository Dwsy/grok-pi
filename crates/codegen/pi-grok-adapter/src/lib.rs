//! Pi core integration boundary for Grok Build.
//!
//! This crate deliberately contains no terminal, widget, drawing, markdown,
//! input, keybinding, or scrollback implementation. It translates Pi's JSONL
//! RPC protocol into standard ACP messages. The production `xai-grok-pager`
//! consumes those messages in the `grok-pi` composition binary.

mod context_projection;
mod model;
mod pi_adapter;
mod prompt_bridge;
mod pi_rpc;
mod queue_bridge;
mod recap_bridge;
mod subagent_projection;
mod todo_bridge;
mod tool_projection;

pub use model::{PiSessionInfo, PiSessionSwitch, scan_local_sessions};
pub use pi_adapter::{PiAgent, PiBootstrap};
pub use pi_rpc::{PiProcess, PiRpc, SpawnConfig};
