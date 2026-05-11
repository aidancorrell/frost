//! Fixture helpers for integration tests.
//!
//! Creates realistic Iceberg table directories with metadata JSON and Avro
//! manifest files for testing the full frost pipeline.

use apache_avro::types::{Record, Value as AvroValue};
use apache_avro::{Schema as AvroSchema, Writer};
use serde_json::json;
use std::path::Path;

pub fn create_healthy_table(root: &Path) {
    let table_dir = root.join("test_ns").join("healthy_table");
    let metadata_dir = table_dir.join("metadata");
    let data_dir = table_dir.join("data");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let data_files: Vec<_> = (0..10)
        .map(|i| {
            let name = format!("part-{:05}.parquet", i);
            let path = data_dir.join(&name);
            std::fs::write(&path, vec![0u8; 1024]).unwrap();
            (
                path.to_string_lossy().to_string(),
                128 * 1024 * 1024_i64,
                1_500_000_i64,
            )
        })
        .collect();

    let manifest_path = metadata_dir.join("snap-1-m0.avro");
    write_manifest_file(&manifest_path, &data_files, &[]);

    let manifest_list_path = metadata_dir.join("snap-1-manifest-list.avro");
    write_manifest_list_file(
        &manifest_list_path,
        &[(manifest_path.to_string_lossy().to_string(), 10, 0)],
    );

    let now_ms = chrono::Utc::now().timestamp_millis();
    let metadata = json!({
        "format-version": 2,
        "table-uuid": "healthy-uuid",
        "location": table_dir.to_string_lossy(),
        "last-updated-ms": now_ms,
        "schemas": [{
            "schema-id": 0, "type": "struct",
            "fields": [
                {"id": 1, "name": "id", "required": true, "type": "long"},
                {"id": 2, "name": "name", "required": false, "type": "string"},
                {"id": 3, "name": "created_at", "required": true, "type": "timestamp"}
            ]
        }],
        "current-schema-id": 0,
        "partition-specs": [{"spec-id": 0, "fields": []}],
        "default-spec-id": 0,
        "sort-orders": [{"order-id": 0, "fields": []}],
        "default-sort-order-id": 0,
        "snapshots": [{
            "snapshot-id": 1,
            "timestamp-ms": now_ms,
            "summary": {"operation": "append"},
            "manifest-list": manifest_list_path.to_string_lossy()
        }],
        "current-snapshot-id": 1
    });

    std::fs::write(
        metadata_dir.join("v1.metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("version-hint.text"), "1").unwrap();
}

pub fn create_small_files_table(root: &Path) {
    let table_dir = root.join("test_ns").join("small_files_table");
    let metadata_dir = table_dir.join("metadata");
    let data_dir = table_dir.join("data");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let mut data_files = Vec::new();
    for i in 0..200 {
        let name = format!("micro-{:05}.parquet", i);
        let path = data_dir.join(&name);
        std::fs::write(&path, vec![0u8; 512]).unwrap();
        data_files.push((path.to_string_lossy().to_string(), 100 * 1024_i64, 500_i64));
    }
    for i in 0..10 {
        let name = format!("normal-{:05}.parquet", i);
        let path = data_dir.join(&name);
        std::fs::write(&path, vec![0u8; 1024]).unwrap();
        data_files.push((
            path.to_string_lossy().to_string(),
            128 * 1024 * 1024_i64,
            1_000_000_i64,
        ));
    }

    let manifest_path = metadata_dir.join("snap-1-m0.avro");
    write_manifest_file(&manifest_path, &data_files, &[]);

    let manifest_list_path = metadata_dir.join("snap-1-manifest-list.avro");
    write_manifest_list_file(
        &manifest_list_path,
        &[(
            manifest_path.to_string_lossy().to_string(),
            data_files.len() as i32,
            0,
        )],
    );

    let now_ms = chrono::Utc::now().timestamp_millis();
    let metadata = json!({
        "format-version": 2,
        "table-uuid": "small-files-uuid",
        "location": table_dir.to_string_lossy(),
        "last-updated-ms": now_ms,
        "schemas": [{
            "schema-id": 0, "type": "struct",
            "fields": [
                {"id": 1, "name": "id", "required": true, "type": "long"},
                {"id": 2, "name": "data", "required": false, "type": "string"}
            ]
        }],
        "current-schema-id": 0,
        "partition-specs": [{"spec-id": 0, "fields": []}],
        "default-spec-id": 0,
        "sort-orders": [{"order-id": 0, "fields": []}],
        "default-sort-order-id": 0,
        "snapshots": [{
            "snapshot-id": 1,
            "timestamp-ms": now_ms,
            "summary": {"operation": "append"},
            "manifest-list": manifest_list_path.to_string_lossy()
        }],
        "current-snapshot-id": 1
    });

    std::fs::write(
        metadata_dir.join("v1.metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("version-hint.text"), "1").unwrap();
}

pub fn create_snapshot_bloat_table(root: &Path) {
    let table_dir = root.join("test_ns").join("snapshot_bloat_table");
    let metadata_dir = table_dir.join("metadata");
    let data_dir = table_dir.join("data");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let data_files: Vec<_> = (0..5)
        .map(|i| {
            let path = data_dir.join(format!("part-{:05}.parquet", i));
            std::fs::write(&path, vec![0u8; 512]).unwrap();
            (
                path.to_string_lossy().to_string(),
                64 * 1024 * 1024_i64,
                500_000_i64,
            )
        })
        .collect();

    let manifest_path = metadata_dir.join("snap-250-m0.avro");
    write_manifest_file(&manifest_path, &data_files, &[]);

    let manifest_list_path = metadata_dir.join("snap-250-manifest-list.avro");
    write_manifest_list_file(
        &manifest_list_path,
        &[(manifest_path.to_string_lossy().to_string(), 5, 0)],
    );

    let now_ms = chrono::Utc::now().timestamp_millis();
    let day_ms: i64 = 86_400_000;
    let snapshots: Vec<_> = (0..250)
        .map(|i| {
            json!({
                "snapshot-id": i + 1,
                "timestamp-ms": now_ms - (i as i64 * day_ms),
                "summary": {"operation": "append"},
                "manifest-list": manifest_list_path.to_string_lossy()
            })
        })
        .collect();

    let metadata = json!({
        "format-version": 2,
        "table-uuid": "bloat-uuid",
        "location": table_dir.to_string_lossy(),
        "last-updated-ms": now_ms,
        "schemas": [{
            "schema-id": 0, "type": "struct",
            "fields": [{"id": 1, "name": "id", "required": true, "type": "long"}]
        }],
        "current-schema-id": 0,
        "partition-specs": [{"spec-id": 0, "fields": []}],
        "default-spec-id": 0,
        "sort-orders": [{"order-id": 0, "fields": []}],
        "default-sort-order-id": 0,
        "snapshots": snapshots,
        "current-snapshot-id": 250
    });

    std::fs::write(
        metadata_dir.join("v1.metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("version-hint.text"), "1").unwrap();
}

pub fn create_orphan_files_table(root: &Path) {
    let table_dir = root.join("test_ns").join("orphan_files_table");
    let metadata_dir = table_dir.join("metadata");
    let data_dir = table_dir.join("data");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let data_files: Vec<_> = (0..5)
        .map(|i| {
            let path = data_dir.join(format!("part-{:05}.parquet", i));
            std::fs::write(&path, vec![0u8; 512]).unwrap();
            (
                path.to_string_lossy().to_string(),
                64 * 1024 * 1024_i64,
                500_000_i64,
            )
        })
        .collect();

    // 15 orphan files.
    for i in 0..15 {
        let path = data_dir.join(format!("orphan-{:05}.parquet", i));
        std::fs::write(&path, vec![0u8; 256]).unwrap();
    }

    let manifest_path = metadata_dir.join("snap-1-m0.avro");
    write_manifest_file(&manifest_path, &data_files, &[]);

    let manifest_list_path = metadata_dir.join("snap-1-manifest-list.avro");
    write_manifest_list_file(
        &manifest_list_path,
        &[(manifest_path.to_string_lossy().to_string(), 5, 0)],
    );

    let now_ms = chrono::Utc::now().timestamp_millis();
    let metadata = json!({
        "format-version": 2,
        "table-uuid": "orphan-uuid",
        "location": table_dir.to_string_lossy(),
        "last-updated-ms": now_ms,
        "schemas": [{
            "schema-id": 0, "type": "struct",
            "fields": [{"id": 1, "name": "id", "required": true, "type": "long"}]
        }],
        "current-schema-id": 0,
        "partition-specs": [{"spec-id": 0, "fields": []}],
        "default-spec-id": 0,
        "sort-orders": [{"order-id": 0, "fields": []}],
        "default-sort-order-id": 0,
        "snapshots": [{
            "snapshot-id": 1,
            "timestamp-ms": now_ms,
            "summary": {"operation": "append"},
            "manifest-list": manifest_list_path.to_string_lossy()
        }],
        "current-snapshot-id": 1
    });

    std::fs::write(
        metadata_dir.join("v1.metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("version-hint.text"), "1").unwrap();
}

pub fn create_schema_drift_table(root: &Path) {
    let table_dir = root.join("test_ns").join("schema_drift_table");
    let metadata_dir = table_dir.join("metadata");
    let data_dir = table_dir.join("data");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let data_files: Vec<_> = (0..3)
        .map(|i| {
            let path = data_dir.join(format!("part-{i}.parquet"));
            std::fs::write(&path, vec![0u8; 256]).unwrap();
            (
                path.to_string_lossy().to_string(),
                64 * 1024 * 1024_i64,
                500_000_i64,
            )
        })
        .collect();

    let manifest_path = metadata_dir.join("snap-1-m0.avro");
    write_manifest_file(&manifest_path, &data_files, &[]);

    let manifest_list_path = metadata_dir.join("snap-1-manifest-list.avro");
    write_manifest_list_file(
        &manifest_list_path,
        &[(manifest_path.to_string_lossy().to_string(), 3, 0)],
    );

    let now_ms = chrono::Utc::now().timestamp_millis();
    let metadata = json!({
        "format-version": 2,
        "table-uuid": "drift-uuid",
        "location": table_dir.to_string_lossy(),
        "last-updated-ms": now_ms,
        "schemas": [
            {
                "schema-id": 0, "type": "struct",
                "fields": [
                    {"id": 1, "name": "id", "required": true, "type": "long"},
                    {"id": 2, "name": "name", "required": false, "type": "string"},
                    {"id": 3, "name": "email", "required": false, "type": "string"},
                    {"id": 4, "name": "age", "required": false, "type": "int"}
                ]
            },
            {
                "schema-id": 1, "type": "struct",
                "fields": [
                    {"id": 1, "name": "id", "required": true, "type": "long"},
                    {"id": 2, "name": "name", "required": false, "type": "string"},
                    {"id": 4, "name": "age", "required": false, "type": "string"},
                    {"id": 5, "name": "address", "required": false, "type": "string"}
                ]
            }
        ],
        "current-schema-id": 1,
        "partition-specs": [{"spec-id": 0, "fields": []}],
        "default-spec-id": 0,
        "sort-orders": [{"order-id": 0, "fields": []}],
        "default-sort-order-id": 0,
        "snapshots": [{
            "snapshot-id": 1,
            "timestamp-ms": now_ms,
            "summary": {"operation": "append"},
            "manifest-list": manifest_list_path.to_string_lossy()
        }],
        "current-snapshot-id": 1
    });

    std::fs::write(
        metadata_dir.join("v1.metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("version-hint.text"), "1").unwrap();
}

// --- Avro writers ---

fn write_manifest_file(
    path: &Path,
    data_files: &[(String, i64, i64)],
    delete_files: &[(String, i64, i64)],
) {
    let schema_str = r#"{
        "type": "record",
        "name": "manifest_entry",
        "fields": [
            {"name": "status", "type": "int"},
            {"name": "snapshot_id", "type": ["null", "long"], "default": null},
            {"name": "data_file", "type": {
                "type": "record",
                "name": "r2",
                "fields": [
                    {"name": "content", "type": "int", "default": 0},
                    {"name": "file_path", "type": "string"},
                    {"name": "file_format", "type": "string"},
                    {"name": "record_count", "type": "long"},
                    {"name": "file_size_in_bytes", "type": "long"}
                ]
            }}
        ]
    }"#;

    let schema = AvroSchema::parse_str(schema_str).unwrap();
    let mut writer = Writer::new(&schema, Vec::new());

    let write_entry =
        |writer: &mut Writer<Vec<u8>>, file_path: &str, size: i64, records: i64, content: i32| {
            let mut record = Record::new(&schema).unwrap();
            record.put("status", AvroValue::Int(1));
            record.put(
                "snapshot_id",
                AvroValue::Union(1, Box::new(AvroValue::Long(1))),
            );

            if let AvroSchema::Record(rec_schema) = &schema {
                let df_field = rec_schema
                    .fields
                    .iter()
                    .find(|f| f.name == "data_file")
                    .unwrap();
                let mut df_record = Record::new(&df_field.schema).unwrap();
                df_record.put("content", AvroValue::Int(content));
                df_record.put("file_path", AvroValue::String(file_path.to_string()));
                df_record.put("file_format", AvroValue::String("PARQUET".to_string()));
                df_record.put("record_count", AvroValue::Long(records));
                df_record.put("file_size_in_bytes", AvroValue::Long(size));
                record.put("data_file", df_record);
            }
            writer.append(record).unwrap();
        };

    for (path, size, records) in data_files {
        write_entry(&mut writer, path, *size, *records, 0);
    }
    for (path, size, records) in delete_files {
        write_entry(&mut writer, path, *size, *records, 1);
    }

    let encoded = writer.into_inner().unwrap();
    std::fs::write(path, encoded).unwrap();
}

fn write_manifest_list_file(path: &Path, manifests: &[(String, i32, i32)]) {
    let schema_str = r#"{
        "type": "record",
        "name": "manifest_file",
        "fields": [
            {"name": "manifest_path", "type": "string"},
            {"name": "manifest_length", "type": "long"},
            {"name": "partition_spec_id", "type": "int"},
            {"name": "content", "type": "int", "default": 0},
            {"name": "added_snapshot_id", "type": "long"},
            {"name": "added_files_count", "type": "int"},
            {"name": "existing_files_count", "type": "int"},
            {"name": "deleted_files_count", "type": "int"},
            {"name": "added_rows_count", "type": "long"},
            {"name": "existing_rows_count", "type": "long"},
            {"name": "deleted_rows_count", "type": "long"}
        ]
    }"#;

    let schema = AvroSchema::parse_str(schema_str).unwrap();
    let mut writer = Writer::new(&schema, Vec::new());

    for (manifest_path, added_count, content) in manifests {
        let mut record = Record::new(&schema).unwrap();
        record.put("manifest_path", AvroValue::String(manifest_path.clone()));
        record.put("manifest_length", AvroValue::Long(4096));
        record.put("partition_spec_id", AvroValue::Int(0));
        record.put("content", AvroValue::Int(*content));
        record.put("added_snapshot_id", AvroValue::Long(1));
        record.put("added_files_count", AvroValue::Int(*added_count));
        record.put("existing_files_count", AvroValue::Int(0));
        record.put("deleted_files_count", AvroValue::Int(0));
        record.put("added_rows_count", AvroValue::Long(0));
        record.put("existing_rows_count", AvroValue::Long(0));
        record.put("deleted_rows_count", AvroValue::Long(0));
        writer.append(record).unwrap();
    }

    let encoded = writer.into_inner().unwrap();
    std::fs::write(path, encoded).unwrap();
}
