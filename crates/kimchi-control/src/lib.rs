//! kimchi-control: the one door into kimchi.
//!
//! * [`registry`] names, validates and runs every command (`family.verb`).
//! * [`session::Session`] holds the open project and its single undo history,
//!   shared by the window, the built-in agent, `kimchi-cli` and `kimchi-mcp`.
//! * [`bridge`] lets the CLI and MCP drive the running app over a
//!   token-protected loopback socket; [`discovery`] tells other lsuite apps
//!   where kimchi is.

pub mod bridge;
pub mod commands;
pub mod diagnostics;
pub mod discovery;
pub mod keymaps;
pub mod registry;
pub mod release_notes;
pub mod renders;
pub mod resolve;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod update;

pub use registry::{Kind, Param, Perm, Spec, call, commands as specs, describe, input_schema, markdown, spec};
pub use session::{CmdResult, CommandRecord, Event, ExportStatus, Location, Session, SessionOptions, Source, ToastKind, UiCall, UiState};
pub use settings::Settings;

#[cfg(test)]
mod tests;
