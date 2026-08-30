use crate::api::common::map_writer_error;
use crate::conf_mgr::{CenterLifeCycle, ConfCenter, ConfMgr, ConfWriterError, ControllerRuntime, SchemaValidator};
use crate::conf_sync::conf_server::ConfigSyncServer;
use axum::http::StatusCode;
use edgion_resources::ResourceKind;
use std::sync::Arc;

/// Admin state containing ConfMgr and SchemaValidator
pub struct AdminState {
    pub conf_mgr: Arc<ConfMgr>,
    pub schema_validator: Arc<SchemaValidator>,
    /// Path to the controller config YAML, for re-reading `[center]` on reload.
    pub config_file: Option<String>,
    /// Shared authorizer (hot-swappable Center policy) used by reload.
    pub authorizer: Arc<crate::authz::Authorizer>,
    /// Notifies the federation supervisor of `[center]` changes on reload.
    pub center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
}

impl AdminState {
    /// Returns `Ok(Arc<ConfigSyncServer>)` if ready, `Err(503)` if not ready.
    pub fn config_sync_server(&self) -> Result<Arc<ConfigSyncServer>, StatusCode> {
        self.conf_mgr
            .config_sync_server()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// Get the ConfCenter from ConfMgr
    pub fn conf_center(&self) -> Arc<dyn ConfCenter> {
        self.conf_mgr.conf_center()
    }

    /// Return the FileSystem PathRegistry, if this controller runs in FileSystem mode.
    /// Returns `None` in Kubernetes mode.
    pub fn conf_dir_registry(&self) -> Option<Arc<crate::conf_mgr::conf_center::file_system::PathRegistry>> {
        self.conf_center().conf_dir_registry()
    }

    /// Get the ControllerRuntime for direct table access
    pub fn runtime(&self) -> Arc<ControllerRuntime> {
        self.conf_mgr.runtime()
    }

    /// Return a complete processed runtime snapshot for effective-state APIs.
    /// Raw storage is intentionally not used because it lacks Controller-derived
    /// fields and internal reference authorization results.
    ///
    /// The snapshot is **unredacted** and may contain controller-resolved
    /// credentials — project the specific non-secret fields you need out of it
    /// and never return it to a client as-is. See `ProcessorObj::list_cached_json`
    /// for the full secret-boundary rationale.
    pub fn runtime_list_resources(&self, kind: ResourceKind) -> Result<Vec<serde_json::Value>, StatusCode> {
        let runtime = self.runtime();
        let processor = runtime
            .processor_registry
            .get(kind.as_str())
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        if !processor.is_ready() {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(processor.list_cached_json())
    }

    pub fn is_ready(&self) -> bool {
        self.conf_mgr.is_ready()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_mgr.is_k8s_mode()
    }

    // ==================== ConfCenter Methods (Storage Layer) ====================
    // Used by /api/v1/... endpoints - reads directly from storage

    /// List all resources of a kind from storage (via ConfCenter)
    pub async fn center_list_resources(
        &self,
        kind: ResourceKind,
        opts: Option<crate::conf_mgr::ListOptions>,
    ) -> Result<(Vec<serde_json::Value>, Option<String>), StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        let result = api.get_list_by_kind(kind_str, opts).await.map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to list resources from storage"
            );
            map_writer_error(e)
        })?;

        let continue_token = result.continue_token;

        // Parse YAML/JSON content to Vec<Value>
        let values: Vec<serde_json::Value> = result
            .items
            .into_iter()
            .filter_map(|entry| {
                serde_yaml::from_str(&entry.content)
                    .or_else(|_| serde_json::from_str(&entry.content))
                    .ok()
            })
            .collect();

        Ok((values, continue_token))
    }

    /// List resources of a kind in a specific namespace from storage (via ConfCenter)
    pub async fn center_list_resources_namespaced(
        &self,
        kind: ResourceKind,
        namespace: &str,
        opts: Option<crate::conf_mgr::ListOptions>,
    ) -> Result<(Vec<serde_json::Value>, Option<String>), StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        let result = api.get_list_by_kind_ns(kind_str, namespace, opts).await.map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                namespace = namespace,
                error = %e,
                "Failed to list namespaced resources from storage"
            );
            map_writer_error(e)
        })?;

        let continue_token = result.continue_token;

        // Parse YAML/JSON content to Vec<Value>
        let values: Vec<serde_json::Value> = result
            .items
            .into_iter()
            .filter_map(|entry| {
                serde_yaml::from_str(&entry.content)
                    .or_else(|_| serde_json::from_str(&entry.content))
                    .ok()
            })
            .collect();

        Ok((values, continue_token))
    }

    /// List metadata-only keys by kind from storage (via ConfCenter).
    ///
    /// Parallel to `center_list_resources`. Used by `/api/v1/keys/...` handlers.
    /// Consumers needing `spec` (Dashboard list pages, Topology, diagnostics)
    /// stay on `center_list_resources*`.
    pub async fn center_list_keys(
        &self,
        kind: ResourceKind,
        opts: Option<crate::conf_mgr::ListOptions>,
    ) -> Result<(Vec<crate::conf_mgr::conf_center::traits::ResourceKey>, Option<String>), StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        let result = api.list_keys_by_kind(kind_str, opts).await.map_err(|e| {
            tracing::debug!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to list keys from storage"
            );
            map_writer_error(e)
        })?;

        Ok((result.items, result.continue_token))
    }

    /// List metadata-only keys by kind+namespace from storage (via ConfCenter).
    pub async fn center_list_keys_namespaced(
        &self,
        kind: ResourceKind,
        namespace: &str,
        opts: Option<crate::conf_mgr::ListOptions>,
    ) -> Result<(Vec<crate::conf_mgr::conf_center::traits::ResourceKey>, Option<String>), StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        let result = api.list_keys_by_kind_ns(kind_str, namespace, opts).await.map_err(|e| {
            tracing::debug!(
                component = "admin_state",
                kind = kind_str,
                namespace = namespace,
                error = %e,
                "Failed to list namespaced keys from storage"
            );
            map_writer_error(e)
        })?;

        Ok((result.items, result.continue_token))
    }

    /// Get a specific resource by namespace and name from storage (via ConfCenter)
    pub async fn center_get_resource(
        &self,
        kind: ResourceKind,
        namespace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        match api.get_one(kind_str, Some(namespace), name).await {
            Ok(content) => {
                let value: serde_json::Value = serde_yaml::from_str(&content)
                    .or_else(|_| serde_json::from_str(&content))
                    .map_err(|e| {
                        tracing::warn!(
                            component = "admin_state",
                            kind = kind_str,
                            namespace = namespace,
                            name = name,
                            error = %e,
                            "Failed to parse resource content"
                        );
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                Ok(Some(value))
            }
            Err(ConfWriterError::NotFound(_)) => Ok(None),
            Err(e) => {
                tracing::warn!(
                    component = "admin_state",
                    kind = kind_str,
                    namespace = namespace,
                    name = name,
                    error = %e,
                    "Failed to get resource from storage"
                );
                Err(map_writer_error(e))
            }
        }
    }

    /// Get a cluster-scoped resource by name from storage (via ConfCenter)
    pub async fn center_get_cluster_resource(
        &self,
        kind: ResourceKind,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let api = self.conf_center();
        let kind_str = kind.as_str();

        match api.get_one(kind_str, None, name).await {
            Ok(content) => {
                let value: serde_json::Value = serde_yaml::from_str(&content)
                    .or_else(|_| serde_json::from_str(&content))
                    .map_err(|e| {
                        tracing::warn!(
                            component = "admin_state",
                            kind = kind_str,
                            name = name,
                            error = %e,
                            "Failed to parse cluster resource content"
                        );
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                Ok(Some(value))
            }
            Err(ConfWriterError::NotFound(_)) => Ok(None),
            Err(e) => {
                tracing::warn!(
                    component = "admin_state",
                    kind = kind_str,
                    name = name,
                    error = %e,
                    "Failed to get cluster resource from storage"
                );
                Err(map_writer_error(e))
            }
        }
    }

    // ==================== ConfigSyncServer Methods (Cache Layer) ====================
    // Used by /configserver/... endpoints - reads from ServerCache

    /// List all resources of a kind from cache (via ConfigSyncServer)
    pub fn cache_list_resources(&self, kind: ResourceKind) -> Result<Vec<serde_json::Value>, StatusCode> {
        let server = self.config_sync_server()?;
        let kind_str = kind.as_str();

        // Redact resolved secret material in admin output. The struct->JSON
        // serialize (which runs the `redact_*` adapters) happens synchronously
        // inside `server.list()`; the guard is a thread-local RAII held across it.
        // conf_sync transport serialization runs on a different path (no guard),
        // so it still ships real values to the gateway.
        let result = {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            server.list(kind_str)
        }
        .map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to list resources from cache"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Parse JSON data to Vec<Value>
        let values: Vec<serde_json::Value> = serde_json::from_slice(&result.data).map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to parse list response from cache"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        Ok(values)
    }

    /// Get a specific resource by namespace and name from cache (via ConfigSyncServer)
    ///
    /// Cost: O(N) full serialize-then-parse via `cache_list_resources` plus a linear scan.
    /// This is intentional: the only callers are `edgion-cli --target server` and the
    /// `/configserver/{kind}` admin endpoint — manual diagnostic paths, not the data plane.
    /// Bypassing the list by exposing `ServerCache::get_by_key` through `WatchObj` would
    /// expand a cross-cutting trait surface for no measurable benefit on this cold path.
    pub fn cache_get_resource(
        &self,
        kind: ResourceKind,
        namespace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.cache_list_resources(kind)?;

        let found = all.into_iter().find(|v| {
            let meta = v.get("metadata");
            let ns_match = meta.and_then(|m| m.get("namespace")).and_then(|n| n.as_str()) == Some(namespace);
            let name_match = meta.and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name);
            ns_match && name_match
        });

        Ok(found)
    }

    /// Get a cluster-scoped resource by name from cache (via ConfigSyncServer)
    ///
    /// Cost: same O(N) full serialize-then-parse pattern as `cache_get_resource`, retained
    /// for the same reason — admin/diagnostic path only.
    pub fn cache_get_cluster_resource(
        &self,
        kind: ResourceKind,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.cache_list_resources(kind)?;

        let found = all
            .into_iter()
            .find(|v| v.get("metadata").and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name));

        Ok(found)
    }
}

/// Standard API response format used by the Controller Admin API.
#[derive(serde::Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Non-fatal advisories about the request (e.g. unrecognized config keys
    /// the schema does not know about). Empty on every
    /// existing call site, so it never appears in the serialized payload
    /// (`skip_serializing_if`) and existing consumers see an unchanged shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok_body(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            warnings: Vec::new(),
        }
    }

    /// Same as [`Self::ok_body`], but attaches non-fatal warnings for the
    /// caller to surface. The write itself already succeeded — this never
    /// changes `success` or the status code (warn-not-reject policy).
    pub fn ok_body_with_warnings(data: T, warnings: Vec<String>) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            warnings,
        }
    }

    pub fn err_body(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
            warnings: Vec::new(),
        }
    }
}

/// List response format with pagination support.
#[derive(serde::Serialize)]
pub struct ListResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<T>>,
    pub count: usize,
    /// Token for fetching the next page. Absent when this is the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ListResponse<T> {
    pub fn success(data: Vec<T>) -> Self {
        Self::success_with_token(data, None)
    }

    pub fn success_with_token(data: Vec<T>, continue_token: Option<String>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data: Some(data),
            count,
            continue_token,
            error: None,
        }
    }
}

#[cfg(test)]
mod api_response_warnings_tests {
    use super::ApiResponse;

    #[test]
    fn empty_warnings_are_absent_from_serialized_payload() {
        let body = ApiResponse::ok_body("HTTPRoute created".to_string());
        let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
        assert!(
            serialized.get("warnings").is_none(),
            "an empty warnings list must not appear in the JSON at all, matching every pre-existing ok_body call site"
        );
    }

    #[test]
    fn non_empty_warnings_are_present_and_ordered() {
        let body = ApiResponse::ok_body_with_warnings(
            "EdgionPlugins created".to_string(),
            vec!["requestPlugins[0].config.denys".to_string()],
        );
        let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
        assert_eq!(serialized["warnings"][0], "requestPlugins[0].config.denys");
    }

    #[test]
    fn err_body_never_carries_warnings() {
        let body: ApiResponse<()> = ApiResponse::err_body("bad request".to_string());
        let serialized = serde_json::to_value(&body).expect("ApiResponse must serialize");
        assert!(serialized.get("warnings").is_none());
    }
}

#[cfg(test)]
mod cache_redact_tests {
    use crate::conf_sync::{ConfigSyncServer, ServerCache};
    use edgion_common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
    use edgion_resources::redact::AdminRedactGuard;
    use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
    use edgion_resources::resources::link_sys::{LinkSys, LinkSysSpec, SystemConfig, WebhookServiceConfig};
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// Build a `ServerCache<LinkSys>` containing one LinkSys whose webhook config
    /// carries a known plaintext value in `resolved_secrets`.
    fn make_link_sys_cache() -> ServerCache<LinkSys> {
        let cache = ServerCache::<LinkSys>::new(10);

        let mut secrets = BTreeMap::new();
        secrets.insert("ns/n:k".to_string(), "supersecret".to_string());
        let mut webhook_config = WebhookServiceConfig::default();
        webhook_config.resolved_secrets = Some(ResolvedSecrets(secrets));

        let link_sys = LinkSys {
            metadata: ObjectMeta {
                name: Some("test-webhook".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Webhook(webhook_config),
            },
            status: None,
        };

        cache.apply_change(ResourceChange::InitAdd, link_sys, Some(1));
        cache
    }

    /// `ListDataSimple.data` is `Bytes` now; tests assert on its UTF-8 view.
    fn data_str(d: &bytes::Bytes) -> &str {
        std::str::from_utf8(d).expect("list payload is serde_json output, always valid UTF-8")
    }

    /// Prove that holding `AdminRedactGuard` across `ConfigSyncServer::list()` causes
    /// `resolved_secrets` to appear as `"[redacted]"` instead of the real value.
    ///
    /// This is the exact code path exercised by `AdminState::cache_list_resources` after
    /// the Task-9 guard placement: `cache_list_resources` calls `server.list(kind_str)`
    /// inside a block holding `_redact`.
    ///
    /// Building a fully initialised `AdminState` (with a ready `ConfigSyncServer`) in a
    /// unit test requires running the full `ConfCenter::init_resources` lifecycle, which
    /// is impractical here.  The guard placement in `cache_list_resources` is therefore
    /// validated at the `ConfigSyncServer` level — the layer the guard directly covers.
    #[test]
    fn configserver_redacts_resolved_secrets() {
        let cache = make_link_sys_cache();
        let server = ConfigSyncServer::new();
        server.register_watch_obj("LinkSys", Arc::new(cache));

        // Without guard: real value must be present (confirms the adapter IS the gate).
        let no_guard = server.list("LinkSys").expect("list without guard");
        assert!(
            data_str(&no_guard.data).contains("supersecret"),
            "without guard: real value must appear — got: {}",
            data_str(&no_guard.data)
        );
        assert!(
            !data_str(&no_guard.data).contains("[redacted]"),
            "without guard: must not redact — got: {}",
            data_str(&no_guard.data)
        );

        // With guard: secret must be redacted and real value must not appear.
        let with_guard = {
            let _redact = AdminRedactGuard::new();
            server.list("LinkSys").expect("list with guard")
        };
        assert!(
            data_str(&with_guard.data).contains("[redacted]"),
            "with guard: must redact — got: {}",
            data_str(&with_guard.data)
        );
        assert!(
            !data_str(&with_guard.data).contains("supersecret"),
            "with guard: real value must not leak — got: {}",
            data_str(&with_guard.data)
        );
    }

    /// The cached transport path ships real secrets; the admin path stays
    /// redacted; neither poisons the other, in either call order.
    #[tokio::test]
    async fn sync_cached_path_ships_real_secrets_and_stays_unpoisoned() {
        // AdminRedactGuard is a thread-local. This test relies on the
        // current-thread runtime (#[tokio::test] default): every poll of
        // list_sync_cached runs on the guard's thread, so redact_active() is
        // observable across the .await. On a multi-thread runtime this test
        // would silently assert nothing.
        let cache = make_link_sys_cache();
        let server = ConfigSyncServer::new();
        server.register_watch_obj("LinkSys", Arc::new(cache));

        // conf_sync transport (cached): real value.
        let synced = server.list_sync_cached("LinkSys").await.expect("cached list");
        assert!(data_str(&synced.data).contains("supersecret"));

        {
            let _redact = AdminRedactGuard::new();
            // Admin path: redacted.
            let admin = server.list("LinkSys").expect("admin list");
            assert!(!data_str(&admin.data).contains("supersecret"));
            assert!(data_str(&admin.data).contains("[redacted]"));
            // Even through the cached method, a redacting caller never sees the
            // warm real-bytes entry.
            let guarded = server.list_sync_cached("LinkSys").await.expect("guarded list");
            assert!(!data_str(&guarded.data).contains("supersecret"));
            assert!(data_str(&guarded.data).contains("[redacted]"));
        }

        // The cache still serves real bytes afterwards (not poisoned).
        let again = server.list_sync_cached("LinkSys").await.expect("cached list again");
        assert!(data_str(&again.data).contains("supersecret"));
    }
}
