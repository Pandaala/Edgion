//! Bounded native Kubernetes lists and metadata-only counts through the existing client.
use super::*;
use crate::conf_mgr::conf_center::source_query::{
    self as query, SourceCursor, MAX_PAGE_SIZE, MAX_SCAN_ENTRIES, MAX_SOURCE_BYTES, SOURCE_TIMEOUT,
};
use crate::conf_mgr::conf_center::traits::{CountConsistency, SourceCount};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Position {
    namespace_index: usize,
    inner_cursor: Option<String>,
}
#[derive(Deserialize)]
struct NativeList {
    metadata: kube::core::ListMeta,
    items: Vec<Value>,
}
struct Scopes {
    resource: ApiResource,
    namespaced: bool,
    namespaces: Vec<Option<String>>,
}
#[derive(Default)]
struct CountBudget {
    requests: usize,
    identities: usize,
}

impl KubernetesStorage {
    pub(super) async fn replace_source(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected: &query::SourcePrecondition,
    ) -> Result<(), ConfWriterError> {
        query::validate_locator(kind, namespace, name)?;
        tokio::time::timeout(SOURCE_TIMEOUT, async {
            let current = self.source_one(kind, namespace, name).await?;
            let current = expected.verify(&current)?;
            if current
                .pointer("/metadata/resourceVersion")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(query::source_error());
            }
            if content.len() > MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            let requested: Value = serde_yaml::from_str(&content).map_err(|_| query::source_error())?;
            let merged = merge_operator_update_values(current, requested).map_err(|_| query::source_error())?;
            let payload = serde_json::to_vec(&merged).map_err(|_| query::source_error())?;
            if payload.len() > MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            let scopes = self.source_scopes(kind, namespace).await?;
            let api: Api<DynamicObject> = if let Some(ns) = namespace {
                Api::namespaced_with(self.client.clone(), ns, &scopes.resource)
            } else {
                Api::all_with(self.client.clone(), &scopes.resource)
            };
            let request = kube::core::Request::new(api.resource_url())
                .replace(name, &Default::default(), payload)
                .map_err(|_| query::source_error())?;
            let _: Value = super::super::source_transport::json(&self.client, request).await?;
            Ok(())
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    pub(super) async fn source_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<String, ConfWriterError> {
        query::validate_locator(kind, namespace, name)?;
        tokio::time::timeout(SOURCE_TIMEOUT, async {
            let scopes = self.source_scopes(kind, namespace).await?;
            let api: Api<DynamicObject> = if let Some(namespace) = namespace {
                Api::namespaced_with(self.client.clone(), namespace, &scopes.resource)
            } else {
                Api::all_with(self.client.clone(), &scopes.resource)
            };
            let request = kube::core::Request::new(api.resource_url())
                .get(name, &Default::default())
                .map_err(|_| query::source_error())?;
            let value: Value = super::super::source_transport::json(&self.client, request).await?;
            let content = serde_json::to_string(&value).map_err(|_| query::source_error())?;
            if content.len() > MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            let entry = ConfEntry {
                kind: kind.into(),
                namespace: namespace.map(str::to_owned),
                name: name.into(),
                content,
            };
            query::document(&entry)?;
            Ok(entry.content)
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    async fn source_scopes(&self, kind: &str, namespace: Option<&str>) -> Result<Scopes, ConfWriterError> {
        let (resource, scope) = self.resolve_api_resource(kind).await?;
        let namespaced = scope == Scope::Namespaced;
        if !namespaced && namespace.is_some() {
            return Err(ConfWriterError::ValidationError(
                "Cluster resource cannot have a namespace".into(),
            ));
        }
        if namespaced && namespace.is_some() && !self.namespace_allowed(namespace) {
            return Err(ConfWriterError::PermissionDenied(
                "Namespace is outside the configured source scope".into(),
            ));
        }
        if self.allowed_namespaces.len() > MAX_SCAN_ENTRIES {
            return Err(query::budget_error());
        }
        let namespaces = if namespaced && namespace.is_none() && !self.allowed_namespaces.is_empty() {
            self.allowed_namespaces.iter().cloned().map(Some).collect()
        } else {
            vec![namespace.map(str::to_owned)]
        };
        Ok(Scopes {
            resource,
            namespaced,
            namespaces,
        })
    }

    async fn native_source_list(
        &self,
        scopes: &Scopes,
        namespace: Option<&str>,
        limit: u32,
        token: Option<&str>,
        metadata_only: bool,
    ) -> Result<NativeList, ConfWriterError> {
        if scopes.namespaced && !self.namespace_allowed(namespace) {
            return Err(ConfWriterError::PermissionDenied(
                "Namespace is outside the configured source scope".into(),
            ));
        }
        let api: Api<DynamicObject> = if let Some(ns) = namespace {
            Api::namespaced_with(self.client.clone(), ns, &scopes.resource)
        } else {
            Api::all_with(self.client.clone(), &scopes.resource)
        };
        let mut params = ListParams::default().limit(limit);
        if let Some(token) = token {
            params = params.continue_token(token);
        }
        let builder = kube::core::Request::new(api.resource_url());
        let request = if metadata_only {
            builder.list_metadata(&params)
        } else {
            builder.list(&params)
        }
        .map_err(|_| query::source_error())?;
        let list: NativeList = super::super::source_transport::json(&self.client, request).await?;
        if list.items.len() > limit as usize {
            return Err(query::source_error());
        }
        for value in &list.items {
            let meta = query::metadata(value)?;
            if meta.name.is_empty()
                || meta.name.len() > 253
                || (scopes.namespaced && meta.namespace.as_deref().is_none_or(str::is_empty))
                || (!scopes.namespaced && meta.namespace.is_some())
                || namespace.is_some_and(|ns| meta.namespace.as_deref() != Some(ns))
                || (!metadata_only
                    && value
                        .get("kind")
                        .is_some_and(|k| k != &Value::String(scopes.resource.kind.clone())))
            {
                return Err(query::source_error());
            }
        }
        if list
            .metadata
            .continue_
            .as_deref()
            .filter(|t| !t.is_empty())
            .is_some_and(|next| Some(next) == token)
        {
            return Err(query::source_error());
        }
        Ok(list)
    }

    async fn source_values(
        &self,
        kind: &str,
        namespace: Option<&str>,
        options: ListOptions,
        metadata_only: bool,
    ) -> Result<ListResult<Value>, ConfWriterError> {
        tokio::time::timeout(SOURCE_TIMEOUT, async {
            if options.limit == 0 || options.limit > MAX_PAGE_SIZE {
                return Err(ConfWriterError::ValidationError("Invalid source page size".into()));
            }
            let scopes = self.source_scopes(kind, namespace).await?;
            let mut cursor = SourceCursor {
                version: 1,
                backend: if metadata_only { "kubernetes-keys" } else { "kubernetes" }.into(),
                generation: self.source_generation.clone(),
                kind: kind.into(),
                namespace: namespace.map(str::to_owned),
                limit: options.limit,
                position: String::new(),
            };
            let mut position: Position = match options.continue_token {
                Some(token) => serde_json::from_str(&SourceCursor::decode(&token, &cursor)?.position)
                    .map_err(|_| query::stale())?,
                None => Position::default(),
            };
            if position.namespace_index >= scopes.namespaces.len()
                || position
                    .inner_cursor
                    .as_ref()
                    .is_some_and(|t| t.is_empty() || t.len() > query::MAX_CURSOR_BYTES)
            {
                return Err(query::stale());
            }
            let mut items = Vec::new();
            let mut bytes = 0usize;
            for _ in 0..16 {
                let ns = scopes.namespaces[position.namespace_index].as_deref();
                let list = self
                    .native_source_list(
                        &scopes,
                        ns,
                        options.limit - items.len() as u32,
                        position.inner_cursor.as_deref(),
                        metadata_only,
                    )
                    .await?;
                let empty = list.items.is_empty();
                for value in list.items {
                    bytes = bytes
                        .checked_add(serde_json::to_vec(&value).map_err(|_| query::source_error())?.len())
                        .ok_or_else(query::budget_error)?;
                    if bytes > MAX_SOURCE_BYTES {
                        return Err(query::budget_error());
                    }
                    items.push(value);
                }
                position.inner_cursor = list.metadata.continue_.filter(|s| !s.is_empty());
                if position.inner_cursor.is_none() {
                    position.namespace_index += 1;
                }
                if position.namespace_index == scopes.namespaces.len() {
                    return Ok(ListResult {
                        items,
                        continue_token: None,
                    });
                }
                if items.len() == options.limit as usize || (empty && position.inner_cursor.is_some()) {
                    break;
                }
            }
            cursor.position = serde_json::to_string(&position).map_err(|_| query::source_error())?;
            Ok(ListResult {
                items,
                continue_token: Some(cursor.encode()?),
            })
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    pub(super) async fn source_page(
        &self,
        kind: &str,
        namespace: Option<&str>,
        options: ListOptions,
    ) -> Result<ListResult, ConfWriterError> {
        let page = self.source_values(kind, namespace, options, false).await?;
        let mut bytes = 0usize;
        let items = page
            .items
            .into_iter()
            .map(|value| {
                let meta = query::metadata(&value)?;
                let content = serde_yaml::to_string(&value).map_err(|_| query::source_error())?;
                bytes = bytes.checked_add(content.len()).ok_or_else(query::budget_error)?;
                if bytes > MAX_SOURCE_BYTES {
                    return Err(query::budget_error());
                }
                Ok(ConfEntry {
                    kind: kind.into(),
                    namespace: meta.namespace,
                    name: meta.name,
                    content,
                })
            })
            .collect::<Result<Vec<_>, ConfWriterError>>()?;
        Ok(ListResult {
            items,
            continue_token: page.continue_token,
        })
    }
    pub(super) async fn source_keys(
        &self,
        kind: &str,
        namespace: Option<&str>,
        options: ListOptions,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        let page = self.source_values(kind, namespace, options, true).await?;
        let items = page
            .items
            .into_iter()
            .map(|value| {
                let part: kube::core::PartialObjectMeta<DynamicObject> =
                    serde_json::from_value(value).map_err(|_| query::source_error())?;
                Ok(project_partial_meta_to_key(kind, part))
            })
            .collect::<Result<Vec<_>, ConfWriterError>>()?;
        Ok(ListResult {
            items,
            continue_token: page.continue_token,
        })
    }

    async fn native_source_count(
        &self,
        scopes: &Scopes,
        ns: Option<&str>,
        budget: &mut CountBudget,
    ) -> Result<u64, ConfWriterError> {
        let mut token = None;
        let mut version = None;
        let mut total = 0u64;
        loop {
            budget.requests += 1;
            if budget.requests > 2048 {
                return Err(query::budget_error());
            }
            let list = self
                .native_source_list(
                    scopes,
                    ns,
                    if version.is_none() { 1 } else { 100 },
                    token.as_deref(),
                    true,
                )
                .await?;
            budget.identities += list.items.len();
            if budget.identities > MAX_SCAN_ENTRIES {
                return Err(query::budget_error());
            }
            let observed = list
                .metadata
                .resource_version
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(query::source_error)?;
            if version.as_deref().is_some_and(|v| v != observed) {
                return Err(query::stale());
            }
            version = Some(observed.to_owned());
            total = total
                .checked_add(list.items.len() as u64)
                .ok_or_else(query::budget_error)?;
            if let Some(remaining) = list.metadata.remaining_item_count {
                let remaining = u64::try_from(remaining).map_err(|_| query::source_error())?;
                return total.checked_add(remaining).ok_or_else(query::budget_error);
            }
            token = list.metadata.continue_.filter(|s| !s.is_empty());
            if token.is_none() {
                return Ok(total);
            }
        }
    }
    pub(super) async fn source_count(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Result<SourceCount, ConfWriterError> {
        tokio::time::timeout(SOURCE_TIMEOUT, async {
            let scopes = self.source_scopes(kind, namespace).await?;
            let mut budget = CountBudget::default();
            for attempt in 0..2 {
                let mut total = 0u64;
                let mut retry = false;
                for ns in &scopes.namespaces {
                    match self.native_source_count(&scopes, ns.as_deref(), &mut budget).await {
                        Ok(count) => total = total.checked_add(count).ok_or_else(query::budget_error)?,
                        Err(ConfWriterError::StalePagination(_)) if attempt == 0 => {
                            retry = true;
                            break;
                        }
                        Err(error) => return Err(error),
                    }
                }
                if retry {
                    continue;
                }
                return Ok(SourceCount {
                    total,
                    observed_at: chrono::Utc::now(),
                    consistency: if scopes.namespaces.len() > 1 {
                        CountConsistency::ObservedScan
                    } else {
                        CountConsistency::SourceSnapshot
                    },
                });
            }
            Err(query::stale())
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/source_query_test.rs"]
mod tests;
