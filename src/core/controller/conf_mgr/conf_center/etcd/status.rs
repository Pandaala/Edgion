//! EtcdStatusHandler — reads/writes resource status to etcd `/status/` prefix.
//!
//! Status key layout: `{key_prefix}/status/{kind}/{namespace}/{name}`
//! Mirrors `FileSystemStatusHandler` but uses etcd KV instead of `.status` files.

use std::sync::Arc;

use serde::Serialize;

use super::super::common::ErrorStatus;
use crate::core::gateway::link_sys::providers::etcd::EtcdLinkClient;

/// Etcd-backed status handler.
pub struct EtcdStatusHandler {
    client: Arc<EtcdLinkClient>,
    key_prefix: String,
}

impl EtcdStatusHandler {
    pub fn new(client: Arc<EtcdLinkClient>, key_prefix: String) -> Self {
        Self { client, key_prefix }
    }

    /// Build the status key for a resource.
    ///
    /// `key` format: `"namespace/name"` or `"name"` (cluster-scoped).
    fn build_status_key(&self, kind: &str, key: &str) -> String {
        // key is in format "namespace/name" or just "name"
        let (ns, name) = if let Some(idx) = key.find('/') {
            (&key[..idx], &key[idx + 1..])
        } else {
            ("_", key)
        };
        format!("{}/status/{}/{}/{}", self.key_prefix, kind, ns, name)
    }

    /// Write native status to etcd in YAML format.
    pub async fn write_native_status<S: Serialize>(&self, kind: &str, key: &str, status: &S) -> std::io::Result<()> {
        let status_key = self.build_status_key(kind, key);
        let yaml =
            serde_yaml::to_string(status).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        self.client
            .put_string(&status_key, &yaml)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        tracing::trace!(
            component = "etcd_status",
            kind = kind,
            key = key,
            "Wrote native status to etcd"
        );
        Ok(())
    }

    /// Write status from a serde_json::Value.
    pub async fn write_status_value(&self, kind: &str, key: &str, status: &serde_json::Value) -> std::io::Result<()> {
        let status_key = self.build_status_key(kind, key);
        let yaml =
            serde_yaml::to_string(status).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        self.client
            .put_string(&status_key, &yaml)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        tracing::trace!(
            component = "etcd_status",
            kind = kind,
            key = key,
            "Wrote status value to etcd"
        );
        Ok(())
    }

    /// Write error status (for parse errors, etc.)
    pub async fn write_error_status(&self, kind: &str, key: &str, reason: &str, message: &str) -> std::io::Result<()> {
        let status = ErrorStatus::error(reason, message);
        self.write_native_status(kind, key, &status).await
    }

    /// Read status as JSON string (for comparison).
    ///
    /// Returns `None` if key doesn't exist or read/parse fails.
    pub async fn read_status_json(&self, kind: &str, key: &str) -> Option<String> {
        let status_key = self.build_status_key(kind, key);

        match self.client.get_string(&status_key).await {
            Ok(Some(yaml_content)) => {
                // Parse YAML → JSON for consistent comparison (same as FileSystem handler)
                match serde_yaml::from_str::<serde_json::Value>(&yaml_content) {
                    Ok(value) => serde_json::to_string(&value).ok(),
                    Err(e) => {
                        tracing::warn!(
                            component = "etcd_status",
                            kind = kind,
                            key = key,
                            error = %e,
                            "Failed to parse status from etcd"
                        );
                        None
                    }
                }
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    component = "etcd_status",
                    kind = kind,
                    key = key,
                    error = %e,
                    "Failed to read status from etcd"
                );
                None
            }
        }
    }

    /// Delete status entry.
    pub async fn delete_status(&self, kind: &str, key: &str) -> std::io::Result<()> {
        let status_key = self.build_status_key(kind, key);
        self.client
            .delete(&status_key)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        tracing::trace!(
            component = "etcd_status",
            kind = kind,
            key = key,
            "Deleted status from etcd"
        );
        Ok(())
    }
}
