//! The MCP server (Hydra `electron/mcp`): the `studio_*` tools over streamable HTTP on
//! `127.0.0.1:<random port>`, used by the manager agent.
//!
//! Security: the server only listens on loopback (rmcp also rejects foreign `Host` headers) and
//! every request needs `Authorization: Bearer <token>`. The token is random per daemon start and
//! lives only in memory and in the manager workspace's `.mcp.json` (mode 0600), which Claude
//! reads (`headers` of an HTTP server entry). Other local users cannot read the file; other
//! processes of the same user can, which is the same trust level as the daemon's own socket.

mod text;
mod tools;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use as_core::AgentManager;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

pub use tools::StudioTools;
pub use workspace::{CLAUDE_MD, SERVER_NAME, mcp_json};

/// A running MCP server.
pub struct McpServer {
    pub port: u16,
    pub token: String,
    pub workspace: PathBuf,
    stop: CancellationToken,
}

/// 244 random bits (two v4 UUIDs; `uuid` draws from the OS CSPRNG).
fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(State(token): State<Arc<str>>, request: Request, next: Next) -> Response {
    let expected = format!("Bearer {token}");
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|v| constant_time_eq(v.as_bytes(), expected.as_bytes()));
    if authorized {
        next.run(request).await
    } else {
        let mut response = Response::new(axum::body::Body::from("Unauthorized"));
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        response
    }
}

impl McpServer {
    /// Bind, write the manager workspace and serve until [`McpServer::stop`].
    pub async fn start(
        manager: AgentManager,
        workspace_dir: PathBuf,
        max_agents: impl Fn() -> usize + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("binding the MCP server")?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        workspace::setup(&workspace_dir, port, &token).context("writing the manager workspace")?;

        let stop = CancellationToken::new();
        let tools = StudioTools::new(manager, max_agents);
        let service: StreamableHttpService<StudioTools, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(tools.clone()),
                Arc::default(),
                StreamableHttpServerConfig::default().with_cancellation_token(stop.child_token()),
            );
        let app = axum::Router::new().nest_service("/mcp", service).layer(
            middleware::from_fn_with_state(Arc::<str>::from(token.as_str()), require_token),
        );
        let shutdown = stop.clone();
        tokio::spawn(async move {
            let served = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
            if let Err(err) = served {
                tracing::warn!(%err, "MCP server stopped");
            }
        });
        tracing::info!(port, "MCP server listening");
        Ok(Self {
            port,
            token,
            workspace: workspace_dir,
            stop,
        })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }
}

#[cfg(test)]
mod tests;
