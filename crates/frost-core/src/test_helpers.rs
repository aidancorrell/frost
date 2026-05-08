//! Test utilities for creating sample metadata.

use crate::metadata::*;
use chrono::Utc;

/// Create a minimal valid `TableMetadata` for testing.
/// Tests should override the fields they care about.
pub fn make_test_metadata() -> TableMetadata {
    TableMetadata {
        table_name: "test_db.test_table".to_string(),
        location: "s3://test-bucket/warehouse/test_db/test_table".to_string(),
        current_schema: Schema {
            schema_id: 0,
            fields: vec![
                Field {
                    id: 1,
                    name: "id".to_string(),
                    field_type: "long".to_string(),
                    required: true,
                },
                Field {
                    id: 2,
                    name: "name".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                },
                Field {
                    id: 3,
                    name: "created_at".to_string(),
                    field_type: "timestamp".to_string(),
                    required: false,
                },
            ],
        },
        schemas: vec![Schema {
            schema_id: 0,
            fields: vec![
                Field {
                    id: 1,
                    name: "id".to_string(),
                    field_type: "long".to_string(),
                    required: true,
                },
                Field {
                    id: 2,
                    name: "name".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                },
                Field {
                    id: 3,
                    name: "created_at".to_string(),
                    field_type: "timestamp".to_string(),
                    required: false,
                },
            ],
        }],
        snapshots: vec![Snapshot {
            snapshot_id: 1,
            parent_snapshot_id: None,
            timestamp_ms: Utc::now().timestamp_millis(),
            operation: Some("append".to_string()),
            summary: Default::default(),
            manifest_list: "s3://test-bucket/metadata/snap-1-manifest-list.avro".to_string(),
            schema_id: Some(0),
        }],
        current_snapshot_id: Some(1),
        partition_spec: PartitionSpec {
            spec_id: 0,
            fields: vec![],
        },
        partition_specs: vec![PartitionSpec {
            spec_id: 0,
            fields: vec![],
        }],
        sort_order: None,
        sort_orders: vec![],
        refs: Default::default(),
        format_version: 2,
        table_uuid: None,
        properties: Default::default(),
        data_files: vec![],
        delete_files: vec![],
        all_storage_paths: vec![],
        metadata_size_bytes: 1024 * 100, // 100 KB
        manifest_stats: Default::default(),
        collected_at: Utc::now(),
    }
}
