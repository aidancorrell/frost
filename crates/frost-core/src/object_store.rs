//! Minimal object-store abstraction for fetching Iceberg metadata blobs.
//!
//! The REST and Glue catalogs need to pull manifest lists and manifests from
//! object storage (typically S3). This module exposes a single async helper,
//! `fetch_bytes(uri)`, that dispatches on URI scheme:
//!
//! - `s3://bucket/key` → `aws-sdk-s3` with the standard credential chain
//!   (only when the `s3` feature is enabled; default on)
//! - `http(s)://...` → `reqwest`
//! - `file:///...` or plain filesystem paths → `tokio::fs`
//!
//! Kept intentionally narrow: callers write bytes to a temp file and hand
//! the path to the existing Avro parsers, so we don't need streaming or
//! range reads yet.

use std::path::{Path, PathBuf};

/// Errors returned when fetching a metadata blob.
#[derive(Debug, thiserror::Error)]
pub enum ObjectStoreError {
    #[error("unsupported URI scheme: {0}")]
    UnsupportedScheme(String),
    #[error("invalid URI: {0}")]
    InvalidUri(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("S3 error: {0}")]
    S3(String),
    #[error("S3 support not compiled in; rebuild with --features s3")]
    S3Disabled,
}

/// Fetch an object by URI and return its bytes.
pub async fn fetch_bytes(uri: &str) -> Result<Vec<u8>, ObjectStoreError> {
    if let Some(stripped) = uri.strip_prefix("s3://") {
        return fetch_s3(stripped).await;
    }
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return fetch_http(uri).await;
    }
    if let Some(path) = uri.strip_prefix("file://") {
        return fetch_local(Path::new(path)).await;
    }
    // Treat anything else as a filesystem path.
    fetch_local(Path::new(uri)).await
}

/// Fetch an object into a temporary file and return the path.
///
/// Useful because the Avro parsers take `&Path`. The returned guard keeps
/// the temp dir alive for the caller's lifetime.
pub async fn fetch_to_tempfile(
    uri: &str,
    filename: &str,
) -> Result<(tempfile::TempDir, PathBuf), ObjectStoreError> {
    let bytes = fetch_bytes(uri).await?;
    let dir = tempfile::TempDir::new()?;
    let path = dir.path().join(filename);
    tokio::fs::write(&path, &bytes).await?;
    Ok((dir, path))
}

async fn fetch_local(path: &Path) -> Result<Vec<u8>, ObjectStoreError> {
    Ok(tokio::fs::read(path).await?)
}

async fn fetch_http(uri: &str) -> Result<Vec<u8>, ObjectStoreError> {
    let resp = reqwest::get(uri)
        .await
        .map_err(|e| ObjectStoreError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(ObjectStoreError::Http(format!(
            "HTTP {} fetching {}",
            resp.status(),
            uri
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ObjectStoreError::Http(e.to_string()))?;
    Ok(bytes.to_vec())
}

#[cfg(feature = "s3")]
async fn fetch_s3(stripped: &str) -> Result<Vec<u8>, ObjectStoreError> {
    let (bucket, key) = stripped
        .split_once('/')
        .ok_or_else(|| ObjectStoreError::InvalidUri(format!("s3://{stripped}")))?;

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    // Path-style addressing works for real S3 and is required for most
    // S3-compatible stores (MinIO, LocalStack) that lack wildcard-DNS for
    // virtual-hosted-style URLs.
    let s3_config = aws_sdk_s3::config::Builder::from(&config)
        .force_path_style(true)
        .build();
    let client = aws_sdk_s3::Client::from_conf(s3_config);

    let resp = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| ObjectStoreError::S3(format!("GetObject s3://{bucket}/{key}: {e}")))?;

    let bytes = resp
        .body
        .collect()
        .await
        .map_err(|e| ObjectStoreError::S3(format!("read body s3://{bucket}/{key}: {e}")))?
        .into_bytes();

    Ok(bytes.to_vec())
}

#[cfg(not(feature = "s3"))]
async fn fetch_s3(_stripped: &str) -> Result<Vec<u8>, ObjectStoreError> {
    Err(ObjectStoreError::S3Disabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn fetch_local_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let bytes = fetch_bytes(path.to_str().unwrap()).await.unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn fetch_file_scheme() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, b"world").await.unwrap();

        let uri = format!("file://{}", path.display());
        let bytes = fetch_bytes(&uri).await.unwrap();
        assert_eq!(bytes, b"world");
    }

    #[tokio::test]
    async fn unknown_path_treated_as_local() {
        let res = fetch_bytes("/definitely/does/not/exist").await;
        assert!(matches!(res, Err(ObjectStoreError::Io(_))));
    }
}
