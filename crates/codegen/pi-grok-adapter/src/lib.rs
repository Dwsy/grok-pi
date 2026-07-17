//! Pi core integration boundary for Grok Build.
//!
//! This crate deliberately contains no terminal, widget, drawing, markdown,
//! input, keybinding, or scrollback implementation. It translates Pi's JSONL
//! RPC protocol into standard ACP messages. The production `xai-grok-pager`
//! consumes those messages in the `grok-pi` composition binary.

mod model;
mod pi_adapter;
mod pi_rpc;

pub use model::{PiSessionInfo, PiSessionSwitch, scan_local_sessions};
pub use pi_adapter::{PiAgent, PiBootstrap};
pub use pi_rpc::{PiProcess, PiRpc, SpawnConfig};
