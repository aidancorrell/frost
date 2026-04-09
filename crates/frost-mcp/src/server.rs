//! MCP server setup and lifecycle.

use crate::tools::ToolHandler;
use frost_core::config::FrostConfig;

/// The frost MCP server.
pub struct FrostMcpServer {
    pub config: FrostConfig,
    pub handler: ToolHandler,
}

impl FrostMcpServer {
    pub fn new(config: FrostConfig) -> Self {
        Self {
            handler: ToolHandler::new(config.clone()),
            config,
        }
    }

    /// Run the server on stdio transport.
    /// This will be implemented in Phase 3 with full MCP protocol handling.
    pub async fn run_stdio(&self) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!("frost MCP server starting on stdio transport");
        tracing::info!(
            "Available tools: check_table, check_catalog, get_fix, get_cost_report, watch_status"
        );

        // Phase 3: Implement MCP protocol over stdin/stdout.
        // Will use rmcp or hand-rolled JSON-RPC 2.0 over stdio.
        tracing::warn!("MCP server not yet implemented — coming in Phase 3");

        Ok(())
    }
}
