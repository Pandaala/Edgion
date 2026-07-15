//! EtcdStorage — CenterApi implementation backed by etcd KV operations.
//!
//! Key layout: `{key_prefix}/conf/{kind}/{namespace}/{name}`
//! Cluster-scoped resources use `_` as namespace.
//!
//! Reuses `EtcdLinkClient` from gateway LinkSys for all etcd operations.

use std::sync::Arc;

use async_trait::async_trait;

use super::leadership_gate::LeadershipGate;
use crate::conf_mgr::conf_center::traits::{CenterApi, ConfEntry, ConfWriterError, ListOptions, ListResult};
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;

/// Etcd-backed configuration storage implementing `CenterApi`.
pub struct EtcdStorage {
    client: Arc<EtcdLinkClient>,
    key_prefix: String,
    /// Pre-computed "{key_prefix}/conf/" to avoid per-call allocation in parse_key.
    cached_conf_prefix: String,
    /// Optional leadership fence. When `Some`, every mutating operation is
    /// rejected unless the gate reports this node is the fresh leader (spec
    /// §4.6). `None` = ungated (FileSystem/K8s never use this type; a bare
    /// `new` keeps the historical no-fence behavior for non-election callers).
    gate: Option<LeadershipGate>,
}

impl EtcdStorage {
    pub fn new(client: Arc<EtcdLinkClient>, key_prefix: String) -> Self {
        let cached_conf_prefix = format!("{}/conf/", key_prefix);
        Self {
            client,
            key_prefix,
            cached_conf_prefix,
            gate: None,
        }
    }

    /// Construct a leadership-gated storage. Every mutating operation checks the
    /// gate first and fails closed with an `EtcdError` when this node is not the
    /// fresh leader (spec §4.6).
    pub fn new_gated(client: Arc<EtcdLinkClient>, key_prefix: String, gate: LeadershipGate) -> Self {
        let cached_conf_prefix = format!("{}/conf/", key_prefix);
        Self {
            client,
            key_prefix,
            cached_conf_prefix,
            gate: Some(gate),
        }
    }

    /// Fail closed when this node is not the fresh leader. A no-op when ungated.
    fn ensure_leader(&self) -> Result<(), ConfWriterError> {
        if let Some(gate) = &self.gate {
            if gate.check().is_err() {
                return Err(ConfWriterError::EtcdError(
                    "not leader: refusing etcd write".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Build the full etcd key for a resource.
    ///
    /// Layout: `{key_prefix}/conf/{kind}/{namespace}/{name}`
    /// Cluster-scoped (namespace=None) uses `_` as placeholder.
    fn build_key(&self, kind: &str, namespace: Option<&str>, name: &str) -> String {
        let ns = namespace.unwrap_or("_");
        format!("{}/conf/{}/{}/{}", self.key_prefix, kind, ns, name)
    }

    /// Build key prefix for listing by kind.
    fn kind_prefix(&self, kind: &str) -> String {
        format!("{}/conf/{}/", self.key_prefix, kind)
    }

    /// Build key prefix for listing by kind + namespace.
    fn kind_ns_prefix(&self, kind: &str, namespace: &str) -> String {
        format!("{}/conf/{}/{}/", self.key_prefix, kind, namespace)
    }

    /// Build key prefix for listing all config resources.
    pub fn conf_prefix(&self) -> String {
        format!("{}/conf/", self.key_prefix)
    }

    /// Parse an etcd key into (kind, namespace, name).
    ///
    /// Returns `None` if the key doesn't match the expected layout.
    pub fn parse_key(&self, key: &str) -> Option<(String, Option<String>, String)> {
        let stripped = key.strip_prefix(&self.cached_conf_prefix)?;
        let parts: Vec<&str> = stripped.splitn(3, '/').collect();
        if parts.len() != 3 {
            return None;
        }
        let ns = if parts[1] == "_" {
            None
        } else {
            Some(parts[1].to_string())
        };
        Some((parts[0].to_string(), ns, parts[2].to_string()))
    }

    /// Map an anyhow error from EtcdLinkClient to ConfWriterError.
    fn map_err(e: anyhow::Error) -> ConfWriterError {
        ConfWriterError::EtcdError(e.to_string())
    }

    /// Convert a list of etcd KVs into a ListResult.
    fn kvs_to_list_result(&self, kvs: Vec<(String, Vec<u8>, i64)>) -> ListResult {
        let items = kvs
            .into_iter()
            .filter_map(|(key, value, _rev)| {
                let (kind, namespace, name) = self.parse_key(&key)?;
                let content = String::from_utf8(value).ok()?;
                Some(ConfEntry {
                    kind,
                    namespace,
                    name,
                    content,
                })
            })
            .collect();

        ListResult {
            items,
            continue_token: None,
        }
    }
}

#[async_trait]
impl CenterApi for EtcdStorage {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.ensure_leader()?;
        let key = self.build_key(kind, namespace, name);
        self.client.put_string(&key, &content).await.map_err(Self::map_err)?;

        tracing::info!(
            component = "etcd_storage",
            event = "resource_set",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource written to etcd"
        );
        Ok(())
    }

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.ensure_leader()?;
        let key = self.build_key(kind, namespace, name);

        let created = self
            .client
            .txn_put_if_absent(&key, content.into_bytes())
            .await
            .map_err(Self::map_err)?;

        if !created {
            return Err(ConfWriterError::AlreadyExists(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        tracing::info!(
            component = "etcd_storage",
            event = "resource_created",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource created in etcd"
        );
        Ok(())
    }

    async fn update_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.ensure_leader()?;
        let key = self.build_key(kind, namespace, name);

        let updated = self
            .client
            .txn_put_if_exists(&key, content.into_bytes())
            .await
            .map_err(Self::map_err)?;

        if !updated {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        tracing::info!(
            component = "etcd_storage",
            event = "resource_updated",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource updated in etcd"
        );
        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        let key = self.build_key(kind, namespace, name);
        self.client
            .get_string(&key)
            .await
            .map_err(Self::map_err)?
            .ok_or_else(|| ConfWriterError::NotFound(format!("{}/{}/{}", kind, namespace.unwrap_or("_"), name)))
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        self.ensure_leader()?;
        let key = self.build_key(kind, namespace, name);
        let deleted = self.client.delete(&key).await.map_err(Self::map_err)?;

        if deleted == 0 {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        tracing::info!(
            component = "etcd_storage",
            event = "resource_deleted",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource deleted from etcd"
        );
        Ok(())
    }

    async fn get_list_by_kind(&self, kind: &str, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        let prefix = self.kind_prefix(kind);
        let kvs = self.client.get_prefix(&prefix).await.map_err(Self::map_err)?;
        Ok(self.kvs_to_list_result(kvs))
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        _opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        let prefix = self.kind_ns_prefix(kind, namespace);
        let kvs = self.client.get_prefix(&prefix).await.map_err(Self::map_err)?;
        Ok(self.kvs_to_list_result(kvs))
    }

    async fn list_all(&self, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        let prefix = self.conf_prefix();
        let kvs = self.client.get_prefix(&prefix).await.map_err(Self::map_err)?;
        Ok(self.kvs_to_list_result(kvs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::conf_center::etcd::leadership_gate::{LeaderState, LeadershipGate};
    use edgion_common::link_sys_core::credential::CredentialState;
    use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;
    use std::time::Duration;

    /// Build + connect an `EtcdLinkClient` against a local etcd, mirroring the
    /// `leader_election.rs` test helper.
    async fn test_link_client() -> EtcdLinkClient {
        let config = EtcdClientConfig {
            endpoints: vec!["http://localhost:2379".to_string()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
        };
        let client = EtcdLinkClient::from_config("default/etcd-storage-test", &config, &CredentialState::NotRequired)
            .expect("from_config must succeed");
        client.init().await.expect("etcd init must connect");
        client
    }

    /// A non-leader (default `is_leader=false`) `LeadershipGate` must fence every
    /// mutating storage op. `set_one` must return an error without touching etcd.
    #[tokio::test]
    #[ignore = "requires a real etcd on localhost:2379"]
    async fn storage_write_blocked_when_not_leader() {
        let client = Arc::new(test_link_client().await);
        // Default LeaderState has is_leader=false, so the gate is closed.
        let st = LeaderState::new(Duration::from_secs(15));
        let gate = LeadershipGate::from_state(st);
        let storage = EtcdStorage::new_gated(client, "/edgion-test-gate".into(), gate);

        let err = storage
            .set_one("EdgionRoute", Some("default"), "r1", "content".to_string())
            .await;
        assert!(err.is_err(), "a non-leader must not write to etcd");
    }
}
