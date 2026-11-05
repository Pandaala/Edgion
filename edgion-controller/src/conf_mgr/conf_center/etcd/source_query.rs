//! Bounded source pages and validated counts at an Etcd snapshot revision.
use super::*;
use crate::conf_mgr::conf_center::source_query as query;
use crate::conf_mgr::conf_center::traits::{CountConsistency, SourceCount};
use edgion_common::link_sys_core::providers::etcd::SourceRangeError;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    revision: i64,
    last_key: String,
}

fn range_error(error: SourceRangeError) -> ConfWriterError {
    match error {
        SourceRangeError::Expired => query::stale(),
        SourceRangeError::Denied => ConfWriterError::PermissionDenied("Etcd source access denied".into()),
        SourceRangeError::Budget => ConfWriterError::SourceBudget,
        SourceRangeError::Timeout => ConfWriterError::SourceTimeout,
        SourceRangeError::Invalid | SourceRangeError::Unavailable => query::source_error(),
    }
}

fn cluster_scope(kind: &str) -> Result<bool, ConfWriterError> {
    let kind = edgion_resources::ResourceKind::from_kind_name(kind).ok_or_else(query::source_error)?;
    Ok(edgion_resources::resource::is_kind_cluster_scoped(kind))
}

impl EtcdStorage {
    pub(super) async fn replace_source(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected: &query::SourcePrecondition,
    ) -> Result<(), ConfWriterError> {
        query::validate_locator(kind, namespace, name)?;
        self.ensure_leader()?;
        tokio::time::timeout(query::SOURCE_TIMEOUT, async {
            let key = self.build_key(kind, namespace, name);
            let (bytes, revision) = self
                .client
                .get_source_value(&key)
                .await
                .map_err(range_error)?
                .ok_or_else(|| ConfWriterError::NotFound("Source object not found".into()))?;
            let current = String::from_utf8(bytes).map_err(|_| query::source_error())?;
            expected.verify(&current)?;
            let merged = merge_operator_update_yaml(&current, &content).map_err(|_| query::source_error())?;
            if merged.len() > query::MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            self.ensure_leader()?;
            if !self
                .client
                .txn_put_if_mod_revision(&key, revision, merged.into_bytes())
                .await
                .map_err(|_| query::source_error())?
            {
                return Err(ConfWriterError::Conflict("Source changed since preview".into()));
            }
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
        tokio::time::timeout(query::SOURCE_TIMEOUT, async {
            let key = self.build_key(kind, namespace, name);
            let (bytes, _) = self
                .client
                .get_source_value(&key)
                .await
                .map_err(range_error)?
                .ok_or_else(|| ConfWriterError::NotFound("Source object not found".into()))?;
            if bytes.len() > query::MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            let mut entry = self.source_identity(&key, kind, namespace)?;
            entry.content = String::from_utf8(bytes).map_err(|_| query::source_error())?;
            query::document(&entry)?;
            Ok(entry.content)
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    fn source_prefix(&self, kind: &str, namespace: Option<&str>) -> Result<String, ConfWriterError> {
        if namespace.is_some_and(|ns| ns.is_empty() || ns == "_" || ns.contains('/') || ns.contains('\0'))
            || (cluster_scope(kind)? && namespace.is_some())
        {
            return Err(ConfWriterError::ValidationError("Invalid source namespace".into()));
        }
        Ok(match namespace {
            Some(namespace) => self.kind_ns_prefix(kind, namespace),
            None => self.kind_prefix(kind),
        })
    }

    fn source_identity(&self, key: &str, kind: &str, namespace: Option<&str>) -> Result<ConfEntry, ConfWriterError> {
        let (stored_kind, stored_namespace, name) = self.parse_key(key).ok_or_else(query::source_error)?;
        if stored_kind != kind
            || name.is_empty()
            || name.contains('/')
            || name.contains('\0')
            || stored_namespace
                .as_ref()
                .is_some_and(|ns| ns.is_empty() || ns.contains('\0'))
            || cluster_scope(kind)? != stored_namespace.is_none()
            || namespace.is_some_and(|ns| stored_namespace.as_deref() != Some(ns))
        {
            return Err(query::source_error());
        }
        Ok(ConfEntry {
            kind: stored_kind,
            namespace: stored_namespace,
            name,
            content: String::new(),
        })
    }

    async fn read_source_page(
        &self,
        kind: &str,
        namespace: Option<&str>,
        prefix: &str,
        position: Option<&Position>,
        limit: u32,
    ) -> Result<(ListResult, Option<Position>, usize), ConfWriterError> {
        let page = self
            .client
            .get_prefix_page(
                prefix,
                position.map(|p| p.last_key.as_str()),
                position.map(|p| p.revision),
                limit,
            )
            .await
            .map_err(range_error)?;
        let mut items = Vec::with_capacity(page.entries.len());
        let mut bytes = 0usize;
        let mut last_key = None;
        for (key, value, _) in page.entries {
            bytes = bytes.checked_add(value.len()).ok_or_else(query::budget_error)?;
            if bytes > query::MAX_SOURCE_BYTES {
                return Err(query::budget_error());
            }
            let mut entry = self.source_identity(&key, kind, namespace)?;
            entry.content = String::from_utf8(value).map_err(|_| query::source_error())?;
            query::document(&entry)?;
            items.push(entry);
            last_key = Some(key);
        }
        let next = if page.more {
            Some(Position {
                revision: page.revision,
                last_key: last_key.ok_or_else(query::source_error)?,
            })
        } else {
            None
        };
        Ok((
            ListResult {
                items,
                continue_token: None,
            },
            next,
            bytes,
        ))
    }

    pub(super) async fn source_page(
        &self,
        kind: &str,
        namespace: Option<&str>,
        options: ListOptions,
    ) -> Result<ListResult, ConfWriterError> {
        tokio::time::timeout(query::SOURCE_TIMEOUT, async {
            if options.limit == 0 || options.limit > query::MAX_PAGE_SIZE {
                return Err(ConfWriterError::ValidationError("Invalid source page size".into()));
            }
            let prefix = self.source_prefix(kind, namespace)?;
            let mut cursor = query::SourceCursor {
                version: 1,
                backend: "etcd".into(),
                generation: self.source_generation.clone(),
                kind: kind.into(),
                namespace: namespace.map(str::to_owned),
                limit: options.limit,
                position: String::new(),
            };
            let position = if let Some(token) = options.continue_token {
                let decoded = query::SourceCursor::decode(&token, &cursor)?;
                let position: Position = serde_json::from_str(&decoded.position).map_err(|_| query::stale())?;
                if position.revision <= 0 || !position.last_key.starts_with(&prefix) {
                    return Err(query::stale());
                }
                self.source_identity(&position.last_key, kind, namespace)
                    .map_err(|_| query::stale())?;
                Some(position)
            } else {
                None
            };
            let (mut page, next, _) = self
                .read_source_page(kind, namespace, &prefix, position.as_ref(), options.limit)
                .await?;
            if let Some(next) = next {
                cursor.position = serde_json::to_string(&next).map_err(|_| query::source_error())?;
                page.continue_token = Some(cursor.encode()?);
            }
            Ok(page)
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }

    pub(super) async fn source_count(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Result<SourceCount, ConfWriterError> {
        tokio::time::timeout(query::SOURCE_TIMEOUT, async {
            let prefix = self.source_prefix(kind, namespace)?;
            let mut scanned = 0usize;
            let mut bytes = 0usize;
            for attempt in 0..2 {
                let mut position = None;
                let mut total = 0u64;
                loop {
                    let (page, next, page_bytes) = match self
                        .read_source_page(kind, namespace, &prefix, position.as_ref(), query::MAX_PAGE_SIZE)
                        .await
                    {
                        Ok(page) => page,
                        Err(ConfWriterError::StalePagination(_)) if attempt == 0 => break,
                        Err(error) => return Err(error),
                    };
                    scanned = scanned.checked_add(page.items.len()).ok_or_else(query::budget_error)?;
                    bytes = bytes.checked_add(page_bytes).ok_or_else(query::budget_error)?;
                    if scanned > query::MAX_SCAN_ENTRIES || bytes > query::MAX_SCAN_BYTES {
                        return Err(query::budget_error());
                    }
                    total = total
                        .checked_add(page.items.len() as u64)
                        .ok_or_else(query::budget_error)?;
                    if next.is_none() {
                        return Ok(SourceCount {
                            total,
                            observed_at: chrono::Utc::now(),
                            consistency: CountConsistency::SourceSnapshot,
                        });
                    }
                    position = next;
                }
            }
            Err(query::stale())
        })
        .await
        .map_err(|_| ConfWriterError::SourceTimeout)?
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/source_query_test.rs"]
mod tests;
