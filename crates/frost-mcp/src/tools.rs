//! MCP tool definitions and handlers.

use frost_core::config::FrostConfig;
use serde::{Deserialize, Serialize};

/// Handles MCP tool invocations by dispatching to frost-core.
pub struct ToolHandler {
    config: FrostConfig,
}

impl ToolHandler {
    pub fn new(config: FrostConfig) -> Self {
        Self { config }
    }

    /// List available tools with their schemas (for MCP tools/list).
    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "check_table".to_string(),
                description: "Run health checks on an Iceberg table. Returns findings with \
                             severity, impact, and fix commands."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "Table identifier (e.g., 'db.events')"
                        },
                        "checks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional list of check IDs to run (default: all)"
                        }
                    },
                    "required": ["table"]
                }),
            },
            ToolDefinition {
                name: "check_catalog".to_string(),
                description: "Summarize health across all tables in a catalog or namespace."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Optional namespace filter"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "get_fix".to_string(),
                description: "Generate an executable fix command (Spark SQL CALL statement) \
                             for a specific health finding."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "Table identifier"
                        },
                        "finding_id": {
                            "type": "string",
                            "description": "Finding ID (e.g., 'small_files', 'snapshot_bloat')"
                        }
                    },
                    "required": ["table", "finding_id"]
                }),
            },
            ToolDefinition {
                name: "get_cost_report".to_string(),
                description: "Estimate monthly cost waste from detected health issues."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "Table identifier"
                        }
                    },
                    "required": ["table"]
                }),
            },
            ToolDefinition {
                name: "watch_status".to_string(),
                description: "Query watch mode state — recent alerts and health trends."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "Optional table identifier (default: all watched tables)"
                        }
                    }
                }),
            },
        ]
    }

    /// Return a reference to the config (used by tool handlers).
    pub fn config(&self) -> &FrostConfig {
        &self.config
    }
}

/// MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
