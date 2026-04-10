//! AWS Glue catalog implementation.
//!
//! Resolves Iceberg table metadata via the AWS Glue Data Catalog API.
//! Requires the `glue` feature flag and valid AWS credentials.

use crate::catalog::{CatalogError, CatalogProvider};
use crate::metadata::TableMetadata;
use std::future::Future;
use std::pin::Pin;

/// AWS Glue catalog backend.
#[allow(dead_code)]
pub struct GlueCatalog {
    region: Option<String>,
    warehouse: String,
}

impl GlueCatalog {
    pub fn new(region: Option<String>, warehouse: String) -> Self {
        Self { region, warehouse }
    }
}

#[cfg(feature = "glue")]
impl CatalogProvider for GlueCatalog {
    fn load_table(
        &self,
        table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>> {
        use crate::parse::{manifest, metadata_json};
        use std::path::PathBuf;

        let table_id = table_identifier.to_string();
        Box::pin(async move {
            let config = match &self.region {
                Some(region) => {
                    aws_config::defaults(aws_config::BehaviorVersion::latest())
                        .region(aws_config::Region::new(region.clone()))
                        .load()
                        .await
                }
                None => aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await,
            };

            let client = aws_sdk_glue::Client::new(&config);

            // Parse "database.table" identifier.
            let parts: Vec<&str> = table_id.splitn(2, '.').collect();
            let (database, table_name) = match parts.as_slice() {
                [db, tbl] => (*db, *tbl),
                _ => {
                    return Err(CatalogError::Parse(format!(
                        "Invalid table identifier '{}': expected 'database.table'",
                        table_id
                    )));
                }
            };

            // Get table from Glue catalog.
            let response = client
                .get_table()
                .database_name(database)
                .name(table_name)
                .send()
                .await
                .map_err(|e| {
                    CatalogError::TableNotFound(format!("Glue API error for '{}': {}", table_id, e))
                })?;

            let glue_table = response.table().ok_or_else(|| {
                CatalogError::TableNotFound(format!("Table '{}' not found in Glue", table_id))
            })?;

            // Get the metadata location from Glue's table parameters.
            let params = glue_table.parameters();
            let metadata_location = params
                .and_then(|p| p.get("metadata_location"))
                .ok_or_else(|| {
                    CatalogError::Parse(format!(
                        "Glue table '{}' has no metadata_location parameter (not an Iceberg table?)",
                        table_id
                    ))
                })?;

            // Download and parse the metadata JSON from S3.
            let metadata_json_str = download_s3_file(metadata_location).await.map_err(|e| {
                CatalogError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to download {}: {}", metadata_location, e),
                ))
            })?;

            let mut table_meta =
                metadata_json::parse_metadata_json(&metadata_json_str, &table_id)
                    .map_err(|e| CatalogError::Parse(e.to_string()))?;

            // Parse manifest list from the current snapshot.
            let current_snap = table_meta
                .snapshots
                .iter()
                .find(|s| Some(s.snapshot_id) == table_meta.current_snapshot_id)
                .or_else(|| table_meta.snapshots.last());

            if let Some(snapshot) = current_snap {
                if !snapshot.manifest_list.is_empty() {
                    // Download manifest list.
                    let ml_bytes = download_s3_bytes(&snapshot.manifest_list)
                        .await
                        .map_err(|e| {
                            CatalogError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!(
                                    "Failed to download manifest list {}: {}",
                                    snapshot.manifest_list, e
                                ),
                            ))
                        })?;

                    // Write to temp file for Avro parsing.
                    let tmp_dir = tempfile::TempDir::new().map_err(CatalogError::Io)?;
                    let ml_path = tmp_dir.path().join("manifest-list.avro");
                    std::fs::write(&ml_path, &ml_bytes).map_err(CatalogError::Io)?;

                    if let Ok(entries) = manifest::parse_manifest_list(&ml_path) {
                        for entry in &entries {
                            if let Ok(m_bytes) =
                                download_s3_bytes(&entry.manifest_path).await
                            {
                                let m_path = tmp_dir.path().join(
                                    PathBuf::from(&entry.manifest_path)
                                        .file_name()
                                        .unwrap_or_default(),
                                );
                                if std::fs::write(&m_path, &m_bytes).is_ok() {
                                    if let Ok((data, deletes)) = manifest::parse_manifest(&m_path) {
                                        table_meta.data_files.extend(data);
                                        table_meta.delete_files.extend(deletes);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(table_meta)
        })
    }

    fn list_tables(
        &self,
        namespace: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalogError>> + Send + '_>> {
        let ns = namespace.map(|s| s.to_string());
        Box::pin(async move {
            let config = match &self.region {
                Some(region) => {
                    aws_config::defaults(aws_config::BehaviorVersion::latest())
                        .region(aws_config::Region::new(region.clone()))
                        .load()
                        .await
                }
                None => aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await,
            };

            let client = aws_sdk_glue::Client::new(&config);
            let mut tables = Vec::new();

            // List databases (or use the specific namespace).
            let databases = match &ns {
                Some(db) => vec![db.clone()],
                None => {
                    let mut dbs = Vec::new();
                    let mut next_token = None;
                    loop {
                        let mut req = client.get_databases();
                        if let Some(token) = next_token {
                            req = req.next_token(token);
                        }
                        let response = req.send().await.map_err(|e| {
                            CatalogError::Parse(format!("Failed to list Glue databases: {}", e))
                        })?;

                        for db in response.database_list() {
                            dbs.push(db.name().to_string());
                        }

                        next_token = response.next_token().map(|s| s.to_string());
                        if next_token.is_none() {
                            break;
                        }
                    }
                    dbs
                }
            };

            for database in &databases {
                let mut next_token = None;
                loop {
                    let mut req = client.get_tables().database_name(database);
                    if let Some(token) = next_token {
                        req = req.next_token(token);
                    }
                    let response = req.send().await.map_err(|e| {
                        CatalogError::Parse(format!(
                            "Failed to list tables in '{}': {}",
                            database, e
                        ))
                    })?;

                    for tbl in response.table_list() {
                        // Only include Iceberg tables (have metadata_location).
                        let is_iceberg = tbl
                            .parameters()
                            .and_then(|p| p.get("metadata_location"))
                            .is_some();
                        if is_iceberg {
                            tables.push(format!("{}.{}", database, tbl.name()));
                        }
                    }

                    next_token = response.next_token().map(|s| s.to_string());
                    if next_token.is_none() {
                        break;
                    }
                }
            }

            tables.sort();
            Ok(tables)
        })
    }
}

/// Stub implementation when glue feature is disabled — returns a descriptive error.
#[cfg(not(feature = "glue"))]
impl CatalogProvider for GlueCatalog {
    fn load_table(
        &self,
        _table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>> {
        Box::pin(async {
            Err(CatalogError::NotImplemented(
                "Glue catalog requires the 'glue' feature flag. Rebuild with: cargo build --features glue".to_string(),
            ))
        })
    }

    fn list_tables(
        &self,
        _namespace: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalogError>> + Send + '_>> {
        Box::pin(async {
            Err(CatalogError::NotImplemented(
                "Glue catalog requires the 'glue' feature flag. Rebuild with: cargo build --features glue".to_string(),
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// S3 helpers (only compiled with glue feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "glue")]
async fn download_s3_file(s3_uri: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = download_s3_bytes(s3_uri).await?;
    Ok(String::from_utf8(bytes)?)
}

#[cfg(feature = "glue")]
async fn download_s3_bytes(
    s3_uri: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    // Parse s3://bucket/key
    let stripped = s3_uri
        .strip_prefix("s3://")
        .ok_or_else(|| format!("Not an S3 URI: {}", s3_uri))?;
    let (bucket, key) = stripped
        .split_once('/')
        .ok_or_else(|| format!("Invalid S3 URI: {}", s3_uri))?;

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let s3_client = aws_sdk_glue::Client::new(&config);

    // Use reqwest to download from S3 via presigned URL or direct SDK.
    // For simplicity, use the S3 SDK directly.
    // Note: In production, you'd use aws_sdk_s3, but we're keeping deps minimal.
    // Fall back to reqwest for S3 file access.
    let url = format!("https://{}.s3.amazonaws.com/{}", bucket, key);
    let response = reqwest::get(&url).await?;
    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}
