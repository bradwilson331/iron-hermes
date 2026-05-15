// Placeholder — full implementation in Task 3 of Plan 32.1-06
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use crate::runner::CronRunnerContext;

pub async fn run_tick_loop(_ctx: Arc<CronRunnerContext>, _cancel: CancellationToken) {
    unimplemented!("run_tick_loop is implemented in Task 3")
}

pub fn prepare_mcp_for_tick(_mcp: &ironhermes_mcp::McpManager) {
    tracing::debug!(
        "prepare_mcp_for_tick: no-op (MCP discovery primitive not yet implemented in ironhermes-mcp; tracked as deferred follow-up)"
    );
}
