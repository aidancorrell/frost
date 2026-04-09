//! Catalog abstraction layer.
//!
//! Defines the trait that all catalog backends implement. The rest of frost-core
//! works against this trait, making it easy to swap between Glue, REST, filesystem,
//! or test fixtures.

use crate::config::CatalogConfig;
use crate::metadata::TableMetadata;
use std::future::Future;
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
/// Reads Iceberg metadata JSON directly from a warehouse directory.
pub struct FilesystemCatalog {
    pub warehouse_path: String,
}

impl FilesystemCatalog {
    pub fn new(warehouse_path: String) -> Self {
        Self { warehouse_path }
    }
}

impl CatalogProvider for FilesystemCatalog {
    fn load_table(
        &self,
        table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>> {
        let msg = format!(
            "filesystem catalog load for '{}' at '{}'",
            table_identifier, self.warehouse_path
        );
        Box::pin(async move { Err(CatalogError::NotImplemented(msg)) })
    }

    fn list_tables(
        &self,
        namespace: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalogError>> + Send + '_>> {
        let msg = format!(
            "filesystem catalog list for namespace {:?} at '{}'",
            namespace, self.warehouse_path
        );
        Box::pin(async move { Err(CatalogError::NotImplemented(msg)) })
    }
}

/// Build a catalog provider from config.
pub fn from_config(config: &CatalogConfig) -> Result<Box<dyn CatalogProvider>, CatalogError> {
    match config {
        CatalogConfig::Filesystem { warehouse } => {
            Ok(Box::new(FilesystemCatalog::new(warehouse.clone())))
        }
        CatalogConfig::Rest { uri, .. } => Err(CatalogError::NotImplemented(format!(
            "REST catalog at '{uri}'"
        ))),
        CatalogConfig::Glue { region, .. } => Err(CatalogError::NotImplemented(format!(
            "Glue catalog in region {:?}",
            region
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
