//! Catalog abstraction layer.
//!
//! Defines the trait that all catalog backends implement. The rest of frost-core
//! works against this trait, making it easy to swap between Glue, REST, filesystem,
//! or test fixtures.

pub mod glue;
pub mod rest;

use crate::config::CatalogConfig;
use crate::metadata::TableMetadata;
use crate::parse::{manifest, metadata_json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

/// Trait for loading Iceberg table metadata from a catalog.
pub trait CatalogProvider: Send + Sync {
    /// Load full metadata for a single table.
    fn load_table(
        &self,
        table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>>;

    /// List all table identifiers in a namespace (or all namespaces if None).
    fn list_tables(
        &self,
        namespace: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalogError>> + Send + '_>>;
}

/// Filesystem-based catalog for local development and testing.
/// Reads Iceberg metadata JSON and manifest files directly from a warehouse directory.
///
/// Expected layout:
/// ```text
/// warehouse/
///   namespace/
///     table_name/
///       metadata/
///         v1.metadata.json
///         v2.metadata.json
///         ...
///         snap-*.avro (manifest lists and manifests)
///       data/
///         *.parquet
/// ```
pub struct FilesystemCatalog {
    pub warehouse_path: PathBuf,
}

impl FilesystemCatalog {
    pub fn new(warehouse_path: impl Into<PathBuf>) -> Self {
        Self {
            warehouse_path: warehouse_path.into(),
        }
    }

    /// Find the latest metadata JSON file for a table.
    fn find_latest_metadata(&self, table_path: &Path) -> Result<PathBuf, CatalogError> {
        let metadata_dir = table_path.join("metadata");
        if !metadata_dir.exists() {
            return Err(CatalogError::TableNotFound(format!(
                "metadata directory not found: {}",
                metadata_dir.display()
            )));
        }

        // Try version-hint.text first.
        let hint_file = metadata_dir.join("version-hint.text");
        if hint_file.exists() {
            let hint = std::fs::read_to_string(&hint_file).map_err(CatalogError::Io)?;
            let version: i32 = hint.trim().parse().map_err(|_| {
                CatalogError::Parse(format!("invalid version hint: {}", hint.trim()))
            })?;
            let versioned = metadata_dir.join(format!("v{}.metadata.json", version));
            if versioned.exists() {
                return Ok(versioned);
            }
        }

        // Fall back to scanning for highest-versioned metadata file.
        let pattern = metadata_dir.join("v*.metadata.json");
        let pattern_str = pattern.to_string_lossy();
        let mut metadata_files: Vec<PathBuf> = glob::glob(&pattern_str)
            .map_err(|e| CatalogError::Parse(format!("glob error: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        if metadata_files.is_empty() {
            // Also try non-versioned metadata.json or files with UUIDs.
            let alt_pattern = metadata_dir.join("*.metadata.json");
            let alt_str = alt_pattern.to_string_lossy();
            metadata_files = glob::glob(&alt_str)
                .map_err(|e| CatalogError::Parse(format!("glob error: {}", e)))?
                .filter_map(|r| r.ok())
                .collect();
        }

        if metadata_files.is_empty() {
            return Err(CatalogError::TableNotFound(format!(
                "no metadata.json found in {}",
                metadata_dir.display()
            )));
        }

        // Sort by version number (extract from filename).
        metadata_files.sort_by(|a, b| {
            let va = extract_version(a);
            let vb = extract_version(b);
            va.cmp(&vb)
        });

        Ok(metadata_files.last().unwrap().clone())
    }

    /// Resolve a table identifier (e.g., "db.events") to a filesystem path.
    fn resolve_table_path(&self, table_identifier: &str) -> PathBuf {
        // Support both "namespace.table" (dot-separated) and "namespace/table" (path).
        let parts: Vec<&str> = table_identifier.split('.').collect();
        let mut path = self.warehouse_path.clone();
        for part in parts {
            path = path.join(part);
        }
        path
    }

    /// Collect all file paths in the data directory (for orphan detection).
    fn list_data_files(&self, table_path: &Path) -> Vec<String> {
        let data_dir = table_path.join("data");
        if !data_dir.exists() {
            return vec![];
        }

        let mut files = Vec::new();
        Self::walk_dir(&data_dir, &mut files);
        files
    }

    fn walk_dir(dir: &Path, files: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    files.push(path.to_string_lossy().to_string());
                } else if path.is_dir() {
                    Self::walk_dir(&path, files);
                }
            }
        }
    }

    /// Calculate total size of all files in the metadata directory.
    fn metadata_dir_size(&self, table_path: &Path) -> u64 {
        let metadata_dir = table_path.join("metadata");
        if !metadata_dir.exists() {
            return 0;
        }

        let mut total = 0u64;
        let mut stack = vec![metadata_dir];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        total += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    } else if path.is_dir() {
                        stack.push(path);
                    }
                }
            }
        }
        total
    }
}

impl CatalogProvider for FilesystemCatalog {
    fn load_table(
        &self,
        table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>> {
        let table_id = table_identifier.to_string();
        let table_path = self.resolve_table_path(&table_id);

        Box::pin(async move {
            // 1. Find and parse the metadata JSON.
            let metadata_file = self.find_latest_metadata(&table_path)?;
            let json_str = std::fs::read_to_string(&metadata_file).map_err(CatalogError::Io)?;
            let mut table_meta = metadata_json::parse_metadata_json(&json_str, &table_id)
                .map_err(|e| CatalogError::Parse(e.to_string()))?;

            // 2. Calculate total metadata size.
            table_meta.metadata_size_bytes = self.metadata_dir_size(&table_path);

            // 3. Find the current snapshot's manifest list and parse it.
            let current_snap = table_meta
                .snapshots
                .iter()
                .find(|s| Some(s.snapshot_id) == table_meta.current_snapshot_id)
                .or_else(|| table_meta.snapshots.last());

            if let Some(snapshot) = current_snap {
                let manifest_list_path_str = &snapshot.manifest_list;
                if !manifest_list_path_str.is_empty() {
                    // Resolve manifest list path — could be absolute or relative.
                    let manifest_list_path = resolve_file_path(
                        manifest_list_path_str,
                        &table_path,
                        &self.warehouse_path,
                    );

                    if manifest_list_path.exists() {
                        match manifest::parse_manifest_list(&manifest_list_path) {
                            Ok(entries) => {
                                table_meta.manifest_stats =
                                    manifest::manifest_stats_from_list(&entries);
                                // 4. Parse each manifest file.
                                for entry in &entries {
                                    let manifest_path = resolve_file_path(
                                        &entry.manifest_path,
                                        &table_path,
                                        &self.warehouse_path,
                                    );

                                    if manifest_path.exists() {
                                        match manifest::parse_manifest(&manifest_path) {
                                            Ok((data, deletes)) => {
                                                table_meta.data_files.extend(data);
                                                table_meta.delete_files.extend(deletes);
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Failed to parse manifest {}: {}",
                                                    manifest_path.display(),
                                                    e
                                                );
                                            }
                                        }
                                    } else {
                                        tracing::debug!(
                                            "Manifest file not found: {}",
                                            manifest_path.display()
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse manifest list {}: {}",
                                    manifest_list_path.display(),
                                    e
                                );
                            }
                        }
                    } else {
                        tracing::debug!(
                            "Manifest list not found: {}",
                            manifest_list_path.display()
                        );
                    }
                }
            }

            // 5. Collect all storage paths for orphan detection.
            table_meta.all_storage_paths = self.list_data_files(&table_path);

            Ok(table_meta)
        })
    }

    fn list_tables(
        &self,
        namespace: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalogError>> + Send + '_>> {
        let ns = namespace.map(|s| s.to_string());
        Box::pin(async move {
            let search_dir = match &ns {
                Some(ns) => self.warehouse_path.join(ns),
                None => self.warehouse_path.clone(),
            };

            if !search_dir.exists() {
                return Ok(vec![]);
            }

            let mut tables = Vec::new();
            self.scan_for_tables(&search_dir, &self.warehouse_path, &mut tables);
            tables.sort();
            Ok(tables)
        })
    }
}

impl FilesystemCatalog {
    fn scan_for_tables(&self, dir: &Path, warehouse_root: &Path, tables: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Check if this directory has a metadata/ subdirectory.
                    let metadata_dir = path.join("metadata");
                    if metadata_dir.exists() && metadata_dir.is_dir() {
                        // This is a table directory. Compute the identifier.
                        if let Ok(relative) = path.strip_prefix(warehouse_root) {
                            let identifier = relative
                                .components()
                                .map(|c| c.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join(".");
                            tables.push(identifier);
                        }
                    } else {
                        // Recurse into subdirectories (could be a namespace).
                        self.scan_for_tables(&path, warehouse_root, tables);
                    }
                }
            }
        }
    }
}

/// Resolve a file path that may be absolute (s3://...) or relative.
/// For filesystem catalogs, we strip s3:// prefixes and resolve relative to
/// the warehouse or table path.
fn resolve_file_path(path_str: &str, table_path: &Path, warehouse_path: &Path) -> PathBuf {
    // If it's a regular filesystem path that exists, use it directly.
    let as_path = PathBuf::from(path_str);
    if as_path.is_absolute() && as_path.exists() {
        return as_path;
    }

    // For s3:// paths, try to resolve relative to warehouse.
    if let Some(stripped) = path_str.strip_prefix("s3://") {
        // s3://bucket/warehouse/... — try to match the warehouse path.
        // Strip the bucket name (first path component).
        let parts: Vec<&str> = stripped.splitn(2, '/').collect();
        if parts.len() == 2 {
            let s3_path = parts[1];
            // Try from warehouse root.
            let candidate = warehouse_path.join(s3_path);
            if candidate.exists() {
                return candidate;
            }
            // Try just the filename in the table's metadata dir.
            if let Some(filename) = PathBuf::from(s3_path).file_name() {
                let candidate = table_path.join("metadata").join(filename);
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }

    // Fall back: try as relative to table path.
    let candidate = table_path.join(path_str);
    if candidate.exists() {
        return candidate;
    }

    // Last resort: try as relative to metadata dir.
    if let Some(filename) = PathBuf::from(path_str).file_name() {
        let candidate = table_path.join("metadata").join(filename);
        if candidate.exists() {
            return candidate;
        }
    }

    // Return the raw path — caller will handle "not found".
    PathBuf::from(path_str)
}

/// Extract a version number from a metadata filename (e.g., "v3.metadata.json" -> 3).
fn extract_version(path: &Path) -> i64 {
    path.file_name()
        .and_then(|f| f.to_str())
        .and_then(|s| s.strip_prefix('v'))
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Build a catalog provider from config.
pub fn from_config(config: &CatalogConfig) -> Result<Box<dyn CatalogProvider>, CatalogError> {
    match config {
        CatalogConfig::Filesystem { warehouse } => Ok(Box::new(FilesystemCatalog::new(warehouse))),
        CatalogConfig::Rest {
            uri, prefix, token, ..
        } => Ok(Box::new(rest::RestCatalog::new(
            uri.clone(),
            prefix.clone(),
            token.clone(),
        ))),
        CatalogConfig::Glue {
            region, warehouse, ..
        } => Ok(Box::new(glue::GlueCatalog::new(
            region.clone(),
            warehouse.clone(),
        ))),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog not yet implemented: {0}")]
    NotImplemented(String),
    #[error("table not found: {0}")]
    TableNotFound(String),
    #[error("catalog I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("metadata parse error: {0}")]
    Parse(String),
}
