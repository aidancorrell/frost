//! frost-core: Iceberg table health check engine.
//!
//! This crate contains all the logic for analyzing Iceberg table metadata,
//! running health checks, estimating cost waste, and generating fix commands.
//! It is the shared engine used by frost-cli and frost-mcp.

pub mod catalog;
pub mod checks;
pub mod config;
pub mod cost;
pub mod engine;
pub mod fix;
pub mod fleet;
pub mod metadata;
pub mod object_store;
pub mod parse;
pub mod report;
pub mod watch;

#[cfg(test)]
pub mod test_helpers;
