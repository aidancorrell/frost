//! Parsers for Iceberg metadata formats.
//!
//! - `metadata_json` — Parses `v*.metadata.json` files (both v1 and v2 format)
//! - `manifest` — Parses manifest list and manifest Avro files

pub mod manifest;
pub mod metadata_json;

#[cfg(test)]
pub mod fixtures;
