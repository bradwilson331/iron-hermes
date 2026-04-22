use crate::config::McpServerConfig;
use crate::security::build_safe_env;
use anyhow::Result;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};

/// Connect to a stdio MCP server. Returns the running service.
///
/// D-19: builds a safe environment using the allowlist (env_clear + build_safe_env).
/// The child process inherits only the safe env keys plus user-specified vars from config.
pub async fn connect_stdio(
    config: &McpServerConfig,
) -> Result<RunningService<RoleClient, ()>> {
    let command = config
        .command
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stdio transport requires 'command' field"))?;

    let safe_env = build_safe_env(&config.env);
    let args = config.args.clone();

    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(command).configure(move |cmd| {
            for arg in &args {
                cmd.arg(arg);
            }
            // D-19: clear host env, then add only safe vars
            cmd.env_clear();
            for (k, v) in &safe_env {
                cmd.env(k, v);
            }
        }),
    )?;

    let client = ().serve(transport).await?;
    Ok(client)
}

/// Connect to an HTTP/StreamableHTTP MCP server.
///
/// Uses `StreamableHttpClientTransport` (reqwest-backed) from rmcp.
/// Requires the `transport-streamable-http-client-reqwest` feature on rmcp.
pub async fn connect_http(
    config: &McpServerConfig,
) -> Result<RunningService<RoleClient, ()>> {
    let url = config
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("HTTP transport requires 'url' field"))?;

    use rmcp::transport::StreamableHttpClientTransport;
    let transport = StreamableHttpClientTransport::from_uri(url.as_str());
    let client = ().serve(transport).await?;
    Ok(client)
}
