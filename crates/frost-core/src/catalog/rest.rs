//! Iceberg REST Catalog backend.
//!
//! Implements the CatalogProvider trait against an Iceberg REST Catalog server
//! (the vendor-neutral HTTP API defined by the Apache Iceberg spec). Supports
//! Polaris, Lakekeeper, Unity Catalog, Gravitino, Nessie, and any other
//! implementation of the Iceberg REST spec.
//!
//! The load-table endpoint returns the full table metadata inline. Manifest
//! lists and manifests are referenced by object-storage paths (typically
//! `s3://...`) and are fetched through `frost_core::object_store`, which uses
//! the standard AWS credential chain for SigV4-signed S3 reads. File-level
//! checks (small_files, partition_skew, delete_pressure, orphan_files) now
//! work on REST-backed warehouses.

use crate::catalog::{CatalogError, CatalogProvider};
use crate::metadata::TableMetadata;
use crate::object_store;
use crate::parse::{manifest, metadata_json};
use reqwest::Client;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct RestCatalog {
    base_url: String,
    prefix: Option<String>,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ListNamespacesResponse {
    namespaces: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ListTablesResponse {
    identifiers: Vec<TableIdentifier>,
}

#[derive(Debug, Deserialize)]
struct TableIdentifier {
    namespace: Vec<String>,
    name: String,
}

impl RestCatalog {
    pub fn new(uri: String, prefix: Option<String>, token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref tok) = token
            && let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {tok}"))
        {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        Self {
            base_url: uri.trim_end_matches('/').to_string(),
            prefix,
            client,
        }
    }

    fn api_base(&self) -> String {
        match &self.prefix {
            Some(prefix) => format!("{}/v1/{}", self.base_url, prefix.trim_matches('/')),
            None => format!("{}/v1", self.base_url),
        }
    }
}

impl CatalogProvider for RestCatalog {
    fn load_table(
        &self,
        table_identifier: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TableMetadata, CatalogError>> + Send + '_>> {
        let table_id = table_identifier.to_string();
        Box::pin(async move {
            let (namespace, table_name) = parse_table_identifier(&table_id)?;

            let url = format!(
                "{}/namespaces/{}/tables/{}",
                self.api_base(),
                encode_namespace(&namespace),
                table_name
            );

            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(|e| CatalogError::Parse(format!("REST request failed: {e}")))?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(CatalogError::TableNotFound(table_id.clone()));
            }

            if !resp.status().is_success() {
                return Err(CatalogError::Parse(format!(
                    "REST catalog returned HTTP {}",
                    resp.status()
                )));
            }

            let body = resp
                .text()
                .await
                .map_err(|e| CatalogError::Parse(format!("failed to read response body: {e}")))?;

            // The REST API load-table response wraps metadata under a "metadata" key.
            let envelope: serde_json::Value =
                serde_json::from_str(&body).map_err(|e| CatalogError::Parse(e.to_string()))?;

            let metadata_value = if envelope.get("metadata").is_some() {
                &envelope["metadata"]
            } else {
                &envelope
            };

            let metadata_str = metadata_value.to_string();
            let mut table_meta = metadata_json::parse_metadata_json(&metadata_str, &table_id)
                .map_err(|e| CatalogError::Parse(e.to_string()))?;

            // Fetch the current snapshot's manifest list and manifests through
            // the object-store abstraction (S3 / GCS / HTTP / local file).
            // Failures here are logged and non-fatal: metadata-only checks
            // still work, but file-level checks need the manifests.
            let current_snap = table_meta
                .snapshots
                .iter()
                .find(|s| Some(s.snapshot_id) == table_meta.current_snapshot_id)
                .or_else(|| table_meta.snapshots.last())
                .cloned();

            if let Some(snapshot) = current_snap
                && !snapshot.manifest_list.is_empty()
            {
                match fetch_and_parse_manifests(&snapshot.manifest_list).await {
                    Ok((data_files, delete_files, manifest_stats)) => {
                        table_meta.data_files.extend(data_files);
                        table_meta.delete_files.extend(delete_files);
                        table_meta.manifest_stats = manifest_stats;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "REST catalog: failed to fetch manifests from {}: {}",
                            snapshot.manifest_list,
                            e
                        );
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
            let namespaces = match &ns {
                Some(n) => vec![n.clone()],
                None => self.list_namespaces().await?,
            };

            let mut tables = Vec::new();
            for namespace in &namespaces {
                let url = format!(
                    "{}/namespaces/{}/tables",
                    self.api_base(),
                    encode_namespace(namespace)
                );

                let resp = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| CatalogError::Parse(format!("REST request failed: {e}")))?;

                if !resp.status().is_success() {
                    tracing::warn!(
                        "Failed to list tables in namespace {}: HTTP {}",
                        namespace,
                        resp.status()
                    );
                    continue;
                }

                let body: ListTablesResponse = resp
                    .json()
                    .await
                    .map_err(|e| CatalogError::Parse(format!("failed to parse response: {e}")))?;

                for ident in body.identifiers {
                    let ns_str = ident.namespace.join(".");
                    tables.push(format!("{}.{}", ns_str, ident.name));
                }
            }

            tables.sort();
            Ok(tables)
        })
    }
}

impl RestCatalog {
    async fn list_namespaces(&self) -> Result<Vec<String>, CatalogError> {
        let url = format!("{}/namespaces", self.api_base());

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CatalogError::Parse(format!("REST request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(CatalogError::Parse(format!(
                "failed to list namespaces: HTTP {}",
                resp.status()
            )));
        }

        let body: ListNamespacesResponse = resp
            .json()
            .await
            .map_err(|e| CatalogError::Parse(format!("failed to parse response: {e}")))?;

        Ok(body
            .namespaces
            .into_iter()
            .map(|parts| parts.join("."))
            .collect())
    }
}

/// Fetch a manifest list from object storage, then each referenced manifest,
/// returning aggregated (data_files, delete_files, manifest_stats).
async fn fetch_and_parse_manifests(
    manifest_list_uri: &str,
) -> Result<
    (
        Vec<crate::metadata::DataFile>,
        Vec<crate::metadata::DeleteFile>,
        crate::metadata::ManifestStats,
    ),
    object_store::ObjectStoreError,
> {
    // Download manifest list to a temp file so the Avro parser can open it.
    let (tmp, ml_path) =
        object_store::fetch_to_tempfile(manifest_list_uri, "manifest-list.avro").await?;
    let entries = manifest::parse_manifest_list(&ml_path)
        .map_err(|e| object_store::ObjectStoreError::Http(format!("parse manifest list: {e}")))?;

    let manifest_stats = manifest::manifest_stats_from_list(&entries);
    let mut data_files = Vec::new();
    let mut delete_files = Vec::new();

    for entry in &entries {
        let filename = std::path::PathBuf::from(&entry.manifest_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "manifest.avro".to_string());

        match object_store::fetch_bytes(&entry.manifest_path).await {
            Ok(bytes) => {
                let m_path = tmp.path().join(&filename);
                if tokio::fs::write(&m_path, &bytes).await.is_ok() {
                    match manifest::parse_manifest(&m_path) {
                        Ok((data, deletes)) => {
                            data_files.extend(data);
                            delete_files.extend(deletes);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "REST catalog: failed to parse manifest {}: {}",
                                entry.manifest_path,
                                e
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "REST catalog: failed to fetch manifest {}: {}",
                    entry.manifest_path,
                    e
                );
            }
        }
    }

    // Keep tmp alive until here.
    drop(tmp);

    Ok((data_files, delete_files, manifest_stats))
}

fn parse_table_identifier(identifier: &str) -> Result<(String, String), CatalogError> {
    let parts: Vec<&str> = identifier.rsplitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(CatalogError::Parse(format!(
            "invalid table identifier '{}': expected 'namespace.table'",
            identifier
        )));
    }
    Ok((parts[1].to_string(), parts[0].to_string()))
}

fn encode_namespace(namespace: &str) -> String {
    // The REST spec uses URL-encoded multipart namespace: "db" -> "db",
    // "catalog.db" -> "catalog%1Fdb" (unit separator). For simple single-level
    // namespaces just return as-is. For dotted namespaces, use %1F separator.
    if namespace.contains('.') {
        namespace.split('.').collect::<Vec<_>>().join("%1F")
    } else {
        namespace.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_table_identifier_simple() {
        let (ns, table) = parse_table_identifier("db.events").unwrap();
        assert_eq!(ns, "db");
        assert_eq!(table, "events");
    }

    #[test]
    fn parse_table_identifier_nested_namespace() {
        let (ns, table) = parse_table_identifier("catalog.db.events").unwrap();
        assert_eq!(ns, "catalog.db");
        assert_eq!(table, "events");
    }

    #[test]
    fn parse_table_identifier_no_namespace() {
        assert!(parse_table_identifier("events").is_err());
    }

    #[test]
    fn encode_namespace_simple() {
        assert_eq!(encode_namespace("db"), "db");
    }

    #[test]
    fn encode_namespace_nested() {
        assert_eq!(encode_namespace("catalog.db"), "catalog%1Fdb");
    }

    #[test]
    fn api_base_no_prefix() {
        let cat = RestCatalog::new("http://localhost:8181".to_string(), None, None);
        assert_eq!(cat.api_base(), "http://localhost:8181/v1");
    }

    #[test]
    fn api_base_with_prefix() {
        let cat = RestCatalog::new(
            "http://localhost:8181".to_string(),
            Some("my_catalog".to_string()),
            None,
        );
        assert_eq!(cat.api_base(), "http://localhost:8181/v1/my_catalog");
    }

    #[test]
    fn api_base_strips_trailing_slash() {
        let cat = RestCatalog::new("http://localhost:8181/".to_string(), None, None);
        assert_eq!(cat.api_base(), "http://localhost:8181/v1");
    }
}
