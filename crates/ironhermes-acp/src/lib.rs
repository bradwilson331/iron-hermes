//! IronHermes ACP (Agent Client Protocol) crate — stdio JSON-RPC server exposing
//! IronHermes to ACP-speaking editors (Zed, VS Code, JetBrains via the `zed-acp-adapter`
//! family), Phase 36.8.
//!
//! Modules:
//! - `entry` (D-07): stdio connection bootstrap. The `acp` subcommand's own
//!   `tracing_subscriber` branch (installed in `ironhermes-cli`) is the only thing that keeps
//!   this module's stdout usage protocol-only.
//! - `handlers`: the three thin-slice JSON-RPC method handlers (`initialize`, `session/new`,
//!   `session/prompt`) proven end-to-end in plan 01, task 1.
//! - `session_manager`: the full ACP session lifecycle (create/get/remove/fork/list/
//!   cleanup, plus close), `StateStore` write-through (D-10), per-session `ApprovalsStore`
//!   (D-14), and the bounded head-aligned resume rehydration algorithm (D-11).
//! - `session_load`: the `session/load` JSON-RPC handler (D-11), a thin translation layer
//!   over `AcpSessionManager::load`.
//! - `probe` (D-05): benign `-32601` handling for unknown-method probes (`ping`/`health`/
//!   `healthcheck`) and any other unrecognized request; unknown notifications get no reply.
//!
//! - `event_bridge` (CLI-05): translates `AgentRuntime::run_turn`'s three per-turn
//!   callbacks (`stream`/`tool_progress`/`tool_result`) into `session/update`
//!   notifications, with stable tool-call-id correlation.
//! - `session_cancel`: the `session/cancel` notification handler — fires the target
//!   session's current per-turn cancellation token.
//! - `approval_bridge` (D-14/D-15): `AcpApprovalGate` — converts agent-runtime approval
//!   requests into `session/request_permission` round trips, fails closed on every
//!   non-approval outcome, and honours `allow_always` for the current session only.
//! - `tool_render` (CLI-07, D-16): maps tool calls to their ACP `ToolKind`, renders shell
//!   command/output content and file edits as reviewable diffs, and classifies workspace
//!   writes that escape the session cwd for the dangerous-op forced-approval path.

pub mod approval_bridge;
pub mod entry;
pub mod event_bridge;
pub mod handlers;
pub mod probe;
pub mod session_cancel;
pub mod session_load;
pub mod session_manager;
pub mod tool_render;

pub use entry::run_acp;
