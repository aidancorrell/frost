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
//! Supports stdio transport (for Claude Code, Cursor, local agents) and
//! will support HTTP/SSE transport (for shared team servers) in a future release.

pub mod server;
pub mod tools;
