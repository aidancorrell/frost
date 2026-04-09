//! frost-mcp: MCP server for agent-first Iceberg table health tools.
//!
//! Exposes frost-core functionality as MCP tools that AI agents can call:
//!
//! - `check_table` — Run health checks on a single table
//! - `check_catalog` — Summarize health across all tables in a catalog
//! - `get_fix` — Generate an executable fix command for a finding
//! - `get_cost_report` — Estimate monthly cost waste
//! - `watch_status` — Query watch mode state (requires watch daemon)
//!
//! Supports stdio and HTTP/SSE transports.

pub mod server;
pub mod tools;
