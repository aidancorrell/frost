//! Parser for Iceberg manifest list and manifest files (Avro format).
//!
//! Manifest lists contain references to manifest files. Each manifest file
//! contains entries describing data files or delete files. Frost reads these
//! to build the complete picture of a table's data and delete files,
//! including per-file column statistics that power sort-compliance and
//! stats-coverage checks.

use crate::metadata::{DataFile, DeleteFile, DeleteType, FileFormat, ManifestStats};
use apache_avro::Reader;
use apache_avro::types::Value as AvroValue;
use std::collections::HashMap;
use std::path::Path;

/// Entry from a manifest list file — reference to a manifest.
#[derive(Debug, Clone)]
pub struct ManifestListEntry {
    pub manifest_path: String,
    pub manifest_length: i64,
    pub partition_spec_id: i32,
    /// 0 = data, 1 = deletes
    pub content: i32,
    pub added_snapshot_id: i64,
    pub added_files_count: i32,
    pub existing_files_count: i32,
    pub deleted_files_count: i32,
    pub added_rows_count: i64,
    pub existing_rows_count: i64,
    pub deleted_rows_count: i64,
}

/// Entry from a manifest file — describes a data or delete file.
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// 0 = existing, 1 = added, 2 = deleted
    pub status: i32,
    pub snapshot_id: Option<i64>,
    pub data_file: ManifestDataFile,
}

#[derive(Debug, Clone)]
pub struct ManifestDataFile {
    /// 0 = data, 1 = position deletes, 2 = equality deletes
    pub content: i32,
    pub file_path: String,
    pub file_format: String,
    pub record_count: i64,
    pub file_size_in_bytes: i64,
    pub partition: HashMap<String, String>,
    pub column_sizes: HashMap<i32, i64>,
    pub value_counts: HashMap<i32, i64>,
    pub null_value_counts: HashMap<i32, i64>,
    pub sort_order_id: Option<i32>,
    pub spec_id: Option<i32>,
    pub equality_ids: Vec<i32>,
}

/// Compute manifest size distribution from a list of manifest list entries.
pub fn manifest_stats_from_list(entries: &[ManifestListEntry]) -> ManifestStats {
    if entries.is_empty() {
        return ManifestStats::default();
    }
    let mut sizes: Vec<u64> = entries
        .iter()
        .map(|e| e.manifest_length.max(0) as u64)
        .collect();
    sizes.sort_unstable();
    let total: u64 = sizes.iter().sum();
    let median = sizes[sizes.len() / 2];
    let max = *sizes.last().unwrap();
    ManifestStats {
        manifest_count: entries.len() as u64,
        manifests_total_bytes: total,
        median_manifest_bytes: median,
        max_manifest_bytes: max,
    }
}

/// Parse a manifest list Avro file and return its entries.
pub fn parse_manifest_list(path: &Path) -> Result<Vec<ManifestListEntry>, ManifestParseError> {
    let file = std::fs::File::open(path).map_err(ManifestParseError::Io)?;
    let reader = Reader::new(file).map_err(ManifestParseError::Avro)?;

    let mut entries = Vec::new();
    for result in reader {
        let value = result.map_err(ManifestParseError::Avro)?;
        if let AvroValue::Record(fields) = value {
            entries.push(parse_manifest_list_record(&fields)?);
        }
    }

    Ok(entries)
}

/// Parse manifest list entries from raw Avro bytes.
pub fn parse_manifest_list_bytes(
    bytes: &[u8],
) -> Result<Vec<ManifestListEntry>, ManifestParseError> {
    let reader = Reader::new(bytes).map_err(ManifestParseError::Avro)?;

    let mut entries = Vec::new();
    for result in reader {
        let value = result.map_err(ManifestParseError::Avro)?;
        if let AvroValue::Record(fields) = value {
            entries.push(parse_manifest_list_record(&fields)?);
        }
    }

    Ok(entries)
}

/// Parse a manifest Avro file and return data file and delete file entries.
///
/// Returns `(data_files, delete_files)`. Only entries with status 0 (existing)
/// or 1 (added) are included — status 2 (deleted) entries are skipped since
/// they represent files that have been removed.
pub fn parse_manifest(path: &Path) -> Result<(Vec<DataFile>, Vec<DeleteFile>), ManifestParseError> {
    let file = std::fs::File::open(path).map_err(ManifestParseError::Io)?;
    let reader = Reader::new(file).map_err(ManifestParseError::Avro)?;

    let mut data_files = Vec::new();
    let mut delete_files = Vec::new();

    for result in reader {
        let value = result.map_err(ManifestParseError::Avro)?;
        if let AvroValue::Record(fields) = value {
            let entry = parse_manifest_record(&fields)?;
            push_entry(entry, &mut data_files, &mut delete_files);
        }
    }

    Ok((data_files, delete_files))
}

/// Parse manifest entries from raw Avro bytes.
pub fn parse_manifest_bytes(
    bytes: &[u8],
) -> Result<(Vec<DataFile>, Vec<DeleteFile>), ManifestParseError> {
    let reader = Reader::new(bytes).map_err(ManifestParseError::Avro)?;

    let mut data_files = Vec::new();
    let mut delete_files = Vec::new();

    for result in reader {
        let value = result.map_err(ManifestParseError::Avro)?;
        if let AvroValue::Record(fields) = value {
            let entry = parse_manifest_record(&fields)?;
            push_entry(entry, &mut data_files, &mut delete_files);
        }
    }

    Ok((data_files, delete_files))
}

fn push_entry(
    entry: ManifestEntry,
    data_files: &mut Vec<DataFile>,
    delete_files: &mut Vec<DeleteFile>,
) {
    if entry.status == 2 {
        return;
    }

    match entry.data_file.content {
        0 => {
            data_files.push(DataFile {
                file_path: entry.data_file.file_path,
                file_size_bytes: entry.data_file.file_size_in_bytes as u64,
                record_count: entry.data_file.record_count as u64,
                partition: entry.data_file.partition,
                file_format: parse_file_format(&entry.data_file.file_format),
                column_sizes: entry.data_file.column_sizes,
                value_counts: entry.data_file.value_counts,
                null_value_counts: entry.data_file.null_value_counts,
                sort_order_id: entry.data_file.sort_order_id,
                spec_id: entry.data_file.spec_id,
            });
        }
        1 => {
            delete_files.push(DeleteFile {
                file_path: entry.data_file.file_path,
                file_size_bytes: entry.data_file.file_size_in_bytes as u64,
                record_count: entry.data_file.record_count as u64,
                delete_type: DeleteType::PositionDelete,
                equality_ids: vec![],
            });
        }
        2 => {
            delete_files.push(DeleteFile {
                file_path: entry.data_file.file_path,
                file_size_bytes: entry.data_file.file_size_in_bytes as u64,
                record_count: entry.data_file.record_count as u64,
                delete_type: DeleteType::EqualityDelete,
                equality_ids: entry.data_file.equality_ids,
            });
        }
        other => {
            tracing::warn!("Unknown data file content type: {}", other);
        }
    }
}

// --- Avro record parsing helpers ---

fn parse_manifest_list_record(
    fields: &[(String, AvroValue)],
) -> Result<ManifestListEntry, ManifestParseError> {
    let map = avro_field_map(fields);

    Ok(ManifestListEntry {
        manifest_path: get_string(&map, "manifest_path")?,
        manifest_length: get_long(&map, "manifest_length").unwrap_or(0),
        partition_spec_id: get_int(&map, "partition_spec_id").unwrap_or(0),
        content: get_int_or_enum(&map, "content").unwrap_or(0),
        added_snapshot_id: get_long(&map, "added_snapshot_id").unwrap_or(0),
        added_files_count: get_int(&map, "added_files_count").unwrap_or(0),
        existing_files_count: get_int(&map, "existing_files_count").unwrap_or(0),
        deleted_files_count: get_int(&map, "deleted_files_count").unwrap_or(0),
        added_rows_count: get_long(&map, "added_rows_count").unwrap_or(0),
        existing_rows_count: get_long(&map, "existing_rows_count").unwrap_or(0),
        deleted_rows_count: get_long(&map, "deleted_rows_count").unwrap_or(0),
    })
}

fn parse_manifest_record(
    fields: &[(String, AvroValue)],
) -> Result<ManifestEntry, ManifestParseError> {
    let map = avro_field_map(fields);

    let status = get_int(&map, "status").unwrap_or(0);
    let snapshot_id = get_long_nullable(&map, "snapshot_id");

    // The data_file field is a nested record.
    let data_file_record = map
        .get("data_file")
        .ok_or_else(|| ManifestParseError::MissingField("data_file".to_string()))?;

    let data_file = match data_file_record {
        AvroValue::Record(df_fields) => parse_data_file_record(df_fields)?,
        _ => {
            return Err(ManifestParseError::MissingField(
                "data_file (not a record)".to_string(),
            ));
        }
    };

    Ok(ManifestEntry {
        status,
        snapshot_id,
        data_file,
    })
}

fn parse_data_file_record(
    fields: &[(String, AvroValue)],
) -> Result<ManifestDataFile, ManifestParseError> {
    let map = avro_field_map(fields);

    let content = get_int_or_enum(&map, "content").unwrap_or(0);
    let file_path = get_string(&map, "file_path")?;
    let file_format = get_string(&map, "file_format").unwrap_or_else(|_| "PARQUET".to_string());
    let record_count = get_long(&map, "record_count").unwrap_or(0);
    let file_size = get_long(&map, "file_size_in_bytes").unwrap_or(0);
    let sort_order_id = get_int(&map, "sort_order_id");
    let spec_id = get_int(&map, "spec_id");

    // Parse partition data — it's a record with fields whose values vary
    // depending on the partition spec.
    let partition = match map.get("partition") {
        Some(AvroValue::Record(pfields)) => {
            let mut parts = HashMap::new();
            for (k, v) in pfields {
                let val_str = avro_value_to_string(v);
                if !val_str.is_empty() {
                    parts.insert(k.clone(), val_str);
                }
            }
            parts
        }
        _ => HashMap::new(),
    };

    // Column statistics — Iceberg encodes these as arrays of {key, value}
    // structs (Avro's "map" type). They may be missing on older writers
    // or wrapped in unions, so missing → empty map is the right behavior.
    let column_sizes = parse_int_long_map(map.get("column_sizes"));
    let value_counts = parse_int_long_map(map.get("value_counts"));
    let null_value_counts = parse_int_long_map(map.get("null_value_counts"));

    // Equality field IDs (only set on equality delete files).
    let equality_ids = match unwrap_union(map.get("equality_ids")) {
        Some(AvroValue::Array(items)) => items.iter().filter_map(avro_value_to_int).collect(),
        _ => vec![],
    };

    Ok(ManifestDataFile {
        content,
        file_path,
        file_format,
        record_count,
        file_size_in_bytes: file_size,
        partition,
        column_sizes,
        value_counts,
        null_value_counts,
        sort_order_id,
        spec_id,
        equality_ids,
    })
}

/// Iceberg encodes int→long maps as Avro arrays of `{key: int, value: long}`
/// records (or as a real Avro map). Handle both shapes; tolerate union wrappers.
fn parse_int_long_map(value: Option<&&AvroValue>) -> HashMap<i32, i64> {
    let mut out = HashMap::new();
    let v = match unwrap_union(value) {
        Some(v) => v,
        None => return out,
    };
    match v {
        AvroValue::Array(items) => {
            for item in items {
                if let AvroValue::Record(fields) = item {
                    let m = avro_field_map(fields);
                    if let (Some(k), Some(val)) = (get_int(&m, "key"), get_long(&m, "value")) {
                        out.insert(k, val);
                    }
                }
            }
        }
        AvroValue::Map(m) => {
            for (k, v) in m {
                if let (Ok(k_int), Some(v_long)) = (k.parse::<i32>(), avro_value_to_long(v)) {
                    out.insert(k_int, v_long);
                }
            }
        }
        _ => {}
    }
    out
}

fn unwrap_union<'a>(value: Option<&&'a AvroValue>) -> Option<&'a AvroValue> {
    match value {
        Some(v) => match v {
            AvroValue::Union(_, inner) => match inner.as_ref() {
                AvroValue::Null => None,
                other => Some(other),
            },
            AvroValue::Null => None,
            other => Some(*other),
        },
        None => None,
    }
}

fn avro_value_to_int(value: &AvroValue) -> Option<i32> {
    match value {
        AvroValue::Int(v) => Some(*v),
        AvroValue::Long(v) => Some(*v as i32),
        AvroValue::Union(_, inner) => avro_value_to_int(inner),
        _ => None,
    }
}

fn avro_value_to_long(value: &AvroValue) -> Option<i64> {
    match value {
        AvroValue::Long(v) => Some(*v),
        AvroValue::Int(v) => Some(*v as i64),
        AvroValue::Union(_, inner) => avro_value_to_long(inner),
        _ => None,
    }
}

// --- Helper functions for extracting values from Avro records ---

fn avro_field_map(fields: &[(String, AvroValue)]) -> HashMap<&str, &AvroValue> {
    fields.iter().map(|(k, v)| (k.as_str(), v)).collect()
}

fn get_string(map: &HashMap<&str, &AvroValue>, key: &str) -> Result<String, ManifestParseError> {
    match map.get(key) {
        Some(AvroValue::String(s)) => Ok(s.clone()),
        Some(AvroValue::Union(_, inner)) => match inner.as_ref() {
            AvroValue::String(s) => Ok(s.clone()),
            AvroValue::Null => Err(ManifestParseError::MissingField(key.to_string())),
            _ => Ok(format!("{:?}", inner)),
        },
        _ => Err(ManifestParseError::MissingField(key.to_string())),
    }
}

fn get_long(map: &HashMap<&str, &AvroValue>, key: &str) -> Option<i64> {
    match map.get(key) {
        Some(AvroValue::Long(v)) => Some(*v),
        Some(AvroValue::Int(v)) => Some(*v as i64),
        Some(AvroValue::Union(_, inner)) => match inner.as_ref() {
            AvroValue::Long(v) => Some(*v),
            AvroValue::Int(v) => Some(*v as i64),
            _ => None,
        },
        _ => None,
    }
}

fn get_long_nullable(map: &HashMap<&str, &AvroValue>, key: &str) -> Option<i64> {
    get_long(map, key)
}

fn get_int(map: &HashMap<&str, &AvroValue>, key: &str) -> Option<i32> {
    match map.get(key) {
        Some(AvroValue::Int(v)) => Some(*v),
        Some(AvroValue::Long(v)) => Some(*v as i32),
        Some(AvroValue::Union(_, inner)) => match inner.as_ref() {
            AvroValue::Int(v) => Some(*v),
            AvroValue::Long(v) => Some(*v as i32),
            _ => None,
        },
        _ => None,
    }
}

fn get_int_or_enum(map: &HashMap<&str, &AvroValue>, key: &str) -> Option<i32> {
    match map.get(key) {
        Some(AvroValue::Int(v)) => Some(*v),
        Some(AvroValue::Enum(idx, _)) => Some(*idx as i32),
        Some(AvroValue::Union(_, inner)) => match inner.as_ref() {
            AvroValue::Int(v) => Some(*v),
            AvroValue::Enum(idx, _) => Some(*idx as i32),
            _ => None,
        },
        _ => None,
    }
}

fn avro_value_to_string(value: &AvroValue) -> String {
    match value {
        AvroValue::String(s) => s.clone(),
        AvroValue::Int(v) => v.to_string(),
        AvroValue::Long(v) => v.to_string(),
        AvroValue::Float(v) => v.to_string(),
        AvroValue::Double(v) => v.to_string(),
        AvroValue::Boolean(v) => v.to_string(),
        AvroValue::Null => String::new(),
        AvroValue::Union(_, inner) => avro_value_to_string(inner),
        other => format!("{:?}", other),
    }
}

fn parse_file_format(format: &str) -> FileFormat {
    match format.to_uppercase().as_str() {
        "PARQUET" => FileFormat::Parquet,
        "AVRO" => FileFormat::Avro,
        "ORC" => FileFormat::Orc,
        _ => FileFormat::Parquet,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestParseError {
    #[error("I/O error reading manifest: {0}")]
    Io(#[from] std::io::Error),
    #[error("Avro parsing error: {0}")]
    Avro(#[from] apache_avro::Error),
    #[error("Missing required field: {0}")]
    MissingField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_stats_handles_empty() {
        let stats = manifest_stats_from_list(&[]);
        assert_eq!(stats.manifest_count, 0);
        assert_eq!(stats.manifests_total_bytes, 0);
    }

    #[test]
    fn manifest_stats_computes_distribution() {
        let entries = vec![
            ManifestListEntry {
                manifest_path: "a".into(),
                manifest_length: 100,
                partition_spec_id: 0,
                content: 0,
                added_snapshot_id: 1,
                added_files_count: 0,
                existing_files_count: 0,
                deleted_files_count: 0,
                added_rows_count: 0,
                existing_rows_count: 0,
                deleted_rows_count: 0,
            },
            ManifestListEntry {
                manifest_path: "b".into(),
                manifest_length: 200,
                partition_spec_id: 0,
                content: 0,
                added_snapshot_id: 1,
                added_files_count: 0,
                existing_files_count: 0,
                deleted_files_count: 0,
                added_rows_count: 0,
                existing_rows_count: 0,
                deleted_rows_count: 0,
            },
            ManifestListEntry {
                manifest_path: "c".into(),
                manifest_length: 1000,
                partition_spec_id: 0,
                content: 0,
                added_snapshot_id: 1,
                added_files_count: 0,
                existing_files_count: 0,
                deleted_files_count: 0,
                added_rows_count: 0,
                existing_rows_count: 0,
                deleted_rows_count: 0,
            },
        ];
        let stats = manifest_stats_from_list(&entries);
        assert_eq!(stats.manifest_count, 3);
        assert_eq!(stats.manifests_total_bytes, 1300);
        assert_eq!(stats.median_manifest_bytes, 200);
        assert_eq!(stats.max_manifest_bytes, 1000);
    }
}
