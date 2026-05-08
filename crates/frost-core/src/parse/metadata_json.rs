//! Parser for Iceberg table metadata JSON files (format-version 1 and 2).
//!
//! Reads a `v*.metadata.json` file and extracts the information frost needs
//! into our internal `TableMetadata` representation.

use crate::metadata::{
    Field, PartitionField, PartitionSpec, Schema, Snapshot, SnapshotRef, SortField, SortOrder,
    TableMetadata,
};
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;

/// Raw Iceberg metadata JSON structure (supports both v1 and v2).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
struct RawMetadata {
    format_version: i32,
    #[serde(default)]
    table_uuid: Option<String>,
    location: String,
    #[serde(default)]
    last_updated_ms: Option<i64>,

    // Table-level properties (write.target-file-size-bytes, etc.).
    #[serde(default)]
    properties: HashMap<String, String>,

    // Schemas — v2 uses `schemas` array + `current-schema-id`.
    // v1 may only have a single `schema` field.
    #[serde(default)]
    schemas: Vec<RawSchema>,
    #[serde(default)]
    schema: Option<RawSchema>,
    #[serde(default)]
    current_schema_id: Option<i32>,

    // Partition specs.
    #[serde(default)]
    partition_specs: Vec<RawPartitionSpec>,
    #[serde(default)]
    partition_spec: Option<Vec<RawPartitionField>>,
    #[serde(default)]
    default_spec_id: Option<i32>,

    // Sort orders.
    #[serde(default)]
    sort_orders: Vec<RawSortOrder>,
    #[serde(default)]
    default_sort_order_id: Option<i32>,

    // Snapshots.
    #[serde(default)]
    snapshots: Vec<RawSnapshot>,
    #[serde(default)]
    current_snapshot_id: Option<i64>,

    // Refs (branches and tags) — Iceberg v2.
    #[serde(default)]
    refs: HashMap<String, RawSnapshotRef>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
struct RawSchema {
    schema_id: Option<i32>,
    #[serde(default)]
    r#type: Option<String>,
    fields: Vec<RawField>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct RawField {
    id: i32,
    name: String,
    required: bool,
    r#type: serde_json::Value, // Can be string or nested struct
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawPartitionSpec {
    spec_id: i32,
    fields: Vec<RawPartitionField>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct RawPartitionField {
    source_id: i32,
    field_id: Option<i32>,
    name: String,
    transform: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawSortOrder {
    order_id: i32,
    fields: Vec<RawSortField>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawSortField {
    source_id: i32,
    transform: String,
    direction: String,
    null_order: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
struct RawSnapshot {
    snapshot_id: i64,
    #[serde(default)]
    parent_snapshot_id: Option<i64>,
    timestamp_ms: i64,
    #[serde(default)]
    summary: HashMap<String, String>,
    #[serde(default)]
    manifest_list: Option<String>,
    #[serde(default)]
    schema_id: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawSnapshotRef {
    snapshot_id: i64,
    #[serde(rename = "type")]
    ref_type: String,
    #[serde(default)]
    max_ref_age_ms: Option<i64>,
    #[serde(default)]
    max_snapshot_age_ms: Option<i64>,
    #[serde(default)]
    min_snapshots_to_keep: Option<i32>,
}

/// Parse an Iceberg metadata JSON string into our internal representation.
///
/// Note: This only populates fields available in the metadata JSON. Data files
/// and delete files must be loaded separately from manifest files.
pub fn parse_metadata_json(
    json_str: &str,
    table_name: &str,
) -> Result<TableMetadata, MetadataParseError> {
    let raw: RawMetadata = serde_json::from_str(json_str).map_err(MetadataParseError::JsonParse)?;

    // Resolve schemas.
    let schemas = if !raw.schemas.is_empty() {
        raw.schemas.iter().map(convert_schema).collect()
    } else if let Some(ref schema) = raw.schema {
        vec![convert_schema(schema)]
    } else {
        vec![]
    };

    let current_schema_id = raw.current_schema_id.unwrap_or(0);
    let current_schema = schemas
        .iter()
        .find(|s| s.schema_id == current_schema_id)
        .cloned()
        .unwrap_or_else(|| {
            schemas.first().cloned().unwrap_or(Schema {
                schema_id: 0,
                fields: vec![],
            })
        });

    // Resolve partition specs (capture full history, not just default).
    let default_spec_id = raw.default_spec_id.unwrap_or(0);
    let partition_specs: Vec<PartitionSpec> = if !raw.partition_specs.is_empty() {
        raw.partition_specs
            .iter()
            .map(convert_partition_spec)
            .collect()
    } else if let Some(ref fields) = raw.partition_spec {
        vec![PartitionSpec {
            spec_id: 0,
            fields: fields.iter().map(convert_partition_field).collect(),
        }]
    } else {
        vec![]
    };

    let partition_spec = partition_specs
        .iter()
        .find(|s| s.spec_id == default_spec_id)
        .cloned()
        .or_else(|| partition_specs.first().cloned())
        .unwrap_or(PartitionSpec {
            spec_id: 0,
            fields: vec![],
        });

    // Resolve sort orders (history + default).
    let sort_orders: Vec<SortOrder> = raw.sort_orders.iter().map(convert_sort_order).collect();

    let default_sort_id = raw.default_sort_order_id.unwrap_or(0);
    let sort_order = sort_orders
        .iter()
        .find(|s| s.order_id == default_sort_id)
        .filter(|s| !s.fields.is_empty())
        .cloned();

    // Convert snapshots (sorted by timestamp).
    let mut snapshots: Vec<Snapshot> = raw.snapshots.iter().map(convert_snapshot).collect();
    snapshots.sort_by_key(|s| s.timestamp_ms);

    // Refs (branches and tags).
    let refs: HashMap<String, SnapshotRef> = raw
        .refs
        .into_iter()
        .map(|(name, r)| {
            (
                name,
                SnapshotRef {
                    snapshot_id: r.snapshot_id,
                    ref_type: r.ref_type,
                    max_ref_age_ms: r.max_ref_age_ms,
                    max_snapshot_age_ms: r.max_snapshot_age_ms,
                    min_snapshots_to_keep: r.min_snapshots_to_keep,
                },
            )
        })
        .collect();

    Ok(TableMetadata {
        table_name: table_name.to_string(),
        location: raw.location,
        format_version: raw.format_version,
        table_uuid: raw.table_uuid,
        properties: raw.properties,
        current_schema,
        schemas,
        snapshots,
        current_snapshot_id: raw.current_snapshot_id,
        partition_spec,
        partition_specs,
        sort_order,
        sort_orders,
        refs,
        // These must be populated separately from manifests:
        data_files: vec![],
        delete_files: vec![],
        all_storage_paths: vec![],
        metadata_size_bytes: json_str.len() as u64,
        manifest_stats: Default::default(),
        collected_at: Utc::now(),
    })
}

fn convert_schema(raw: &RawSchema) -> Schema {
    Schema {
        schema_id: raw.schema_id.unwrap_or(0),
        fields: raw.fields.iter().map(convert_field).collect(),
    }
}

fn convert_field(raw: &RawField) -> Field {
    let field_type = match &raw.r#type {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    Field {
        id: raw.id,
        name: raw.name.clone(),
        field_type,
        required: raw.required,
    }
}

fn convert_partition_spec(raw: &RawPartitionSpec) -> PartitionSpec {
    PartitionSpec {
        spec_id: raw.spec_id,
        fields: raw.fields.iter().map(convert_partition_field).collect(),
    }
}

fn convert_partition_field(raw: &RawPartitionField) -> PartitionField {
    PartitionField {
        source_id: raw.source_id,
        field_id: raw.field_id.unwrap_or(1000 + raw.source_id),
        name: raw.name.clone(),
        transform: raw.transform.clone(),
    }
}

fn convert_sort_order(raw: &RawSortOrder) -> SortOrder {
    SortOrder {
        order_id: raw.order_id,
        fields: raw
            .fields
            .iter()
            .map(|f| SortField {
                source_id: f.source_id,
                transform: f.transform.clone(),
                direction: f.direction.clone(),
                null_order: f.null_order.clone(),
            })
            .collect(),
    }
}

fn convert_snapshot(raw: &RawSnapshot) -> Snapshot {
    let operation = raw.summary.get("operation").cloned();
    Snapshot {
        snapshot_id: raw.snapshot_id,
        parent_snapshot_id: raw.parent_snapshot_id,
        timestamp_ms: raw.timestamp_ms,
        operation,
        summary: raw.summary.clone(),
        manifest_list: raw.manifest_list.clone().unwrap_or_default(),
        schema_id: raw.schema_id,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataParseError {
    #[error("failed to parse metadata JSON: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("missing required field: {0}")]
    MissingField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_V2_METADATA: &str = r#"{
        "format-version": 2,
        "table-uuid": "test-uuid-1234",
        "location": "s3://test-bucket/warehouse/db/events",
        "last-updated-ms": 1712700000000,
        "properties": {
            "write.target-file-size-bytes": "536870912",
            "write.distribution-mode": "hash"
        },
        "schemas": [
            {
                "schema-id": 0,
                "type": "struct",
                "fields": [
                    {"id": 1, "name": "id", "required": true, "type": "long"},
                    {"id": 2, "name": "data", "required": false, "type": "string"}
                ]
            },
            {
                "schema-id": 1,
                "type": "struct",
                "fields": [
                    {"id": 1, "name": "id", "required": true, "type": "long"},
                    {"id": 2, "name": "data", "required": false, "type": "string"},
                    {"id": 3, "name": "ts", "required": true, "type": "timestamp"}
                ]
            }
        ],
        "current-schema-id": 1,
        "partition-specs": [
            {
                "spec-id": 0,
                "fields": [
                    {"source-id": 3, "field-id": 1000, "name": "ts_day", "transform": "day"}
                ]
            },
            {
                "spec-id": 1,
                "fields": [
                    {"source-id": 3, "field-id": 1000, "name": "ts_hour", "transform": "hour"}
                ]
            }
        ],
        "default-spec-id": 1,
        "sort-orders": [
            {
                "order-id": 1,
                "fields": [
                    {"source-id": 1, "transform": "identity", "direction": "asc", "null-order": "nulls-first"}
                ]
            }
        ],
        "default-sort-order-id": 1,
        "snapshots": [
            {
                "snapshot-id": 100,
                "timestamp-ms": 1712600000000,
                "summary": {"operation": "append", "added-data-files": "5"},
                "manifest-list": "s3://test-bucket/metadata/snap-100-m0.avro"
            },
            {
                "snapshot-id": 200,
                "parent-snapshot-id": 100,
                "timestamp-ms": 1712700000000,
                "summary": {"operation": "overwrite", "added-data-files": "3"},
                "manifest-list": "s3://test-bucket/metadata/snap-200-m0.avro"
            }
        ],
        "current-snapshot-id": 200,
        "refs": {
            "main": {"snapshot-id": 200, "type": "branch"},
            "audit": {"snapshot-id": 100, "type": "branch", "max-ref-age-ms": 604800000}
        }
    }"#;

    #[test]
    fn parse_v2_metadata() {
        let meta = parse_metadata_json(SAMPLE_V2_METADATA, "db.events").unwrap();

        assert_eq!(meta.table_name, "db.events");
        assert_eq!(meta.format_version, 2);
        assert_eq!(meta.table_uuid.as_deref(), Some("test-uuid-1234"));
        assert_eq!(meta.location, "s3://test-bucket/warehouse/db/events");
        assert_eq!(meta.schemas.len(), 2);
        assert_eq!(meta.current_schema.schema_id, 1);
        assert_eq!(meta.current_schema.fields.len(), 3);
        assert_eq!(meta.snapshots.len(), 2);
        assert_eq!(meta.current_snapshot_id, Some(200));
        assert_eq!(meta.partition_spec.fields.len(), 1);
        assert_eq!(meta.partition_spec.fields[0].name, "ts_hour");
        assert_eq!(meta.partition_specs.len(), 2);
        assert!(meta.sort_order.is_some());
        assert_eq!(meta.sort_order.unwrap().fields.len(), 1);
        assert_eq!(
            meta.properties
                .get("write.target-file-size-bytes")
                .map(|s| s.as_str()),
            Some("536870912"),
        );
        assert_eq!(meta.refs.len(), 2);
        assert_eq!(meta.refs.get("main").unwrap().snapshot_id, 200);
        assert_eq!(meta.refs.get("audit").unwrap().ref_type, "branch");
        // Snapshot operation pulled from summary.
        let latest = meta
            .snapshots
            .iter()
            .max_by_key(|s| s.timestamp_ms)
            .unwrap();
        assert_eq!(latest.operation.as_deref(), Some("overwrite"));
        assert_eq!(latest.parent_snapshot_id, Some(100));
    }

    #[test]
    fn parse_v1_metadata() {
        let json = r#"{
            "format-version": 1,
            "location": "s3://bucket/table",
            "schema": {
                "fields": [
                    {"id": 1, "name": "col1", "required": true, "type": "int"}
                ]
            },
            "partition-spec": [
                {"source-id": 1, "name": "col1_bucket", "transform": "bucket[16]"}
            ],
            "snapshots": [
                {
                    "snapshot-id": 1,
                    "timestamp-ms": 1700000000000,
                    "summary": {},
                    "manifest-list": "s3://bucket/metadata/snap-1-m0.avro"
                }
            ],
            "current-snapshot-id": 1
        }"#;

        let meta = parse_metadata_json(json, "test.table").unwrap();
        assert_eq!(meta.format_version, 1);
        assert_eq!(meta.schemas.len(), 1);
        assert_eq!(meta.schemas[0].fields[0].name, "col1");
        assert_eq!(meta.partition_spec.fields[0].transform, "bucket[16]");
        assert_eq!(meta.partition_specs.len(), 1);
        assert_eq!(meta.snapshots.len(), 1);
        // Refs missing on v1 — should be empty, not error.
        assert!(meta.refs.is_empty());
    }
}
