//! Bounded source-query contracts. Tokens identify a query, never authorize it.
use super::traits::{ConfEntry, ConfWriterError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_PAGE_SIZE: u32 = 10;
pub const MAX_PAGE_SIZE: u32 = 100;
pub const MAX_CURSOR_BYTES: usize = 8192;
pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SCAN_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_SCAN_ENTRIES: usize = 100_000;
pub fn source_error() -> ConfWriterError {
    ConfWriterError::InternalError("Source document is invalid or cannot be read".into())
}
pub fn budget_error() -> ConfWriterError {
    ConfWriterError::SourceBudget
}
pub fn stale() -> ConfWriterError {
    ConfWriterError::StalePagination("Source query changed; restart from the first page".into())
}

/// Parse without logging source contents or silently omitting malformed objects.
pub fn document(entry: &ConfEntry) -> Result<serde_json::Value, ConfWriterError> {
    let value: serde_json::Value = serde_yaml::from_str(&entry.content).map_err(|_| source_error())?;
    let meta = metadata(&value)?;
    if !value.is_object()
        || value
            .get("kind")
            .is_some_and(|kind| kind != &serde_json::Value::String(entry.kind.clone()))
        || meta.name != entry.name
        || meta.namespace != entry.namespace
    {
        return Err(source_error());
    }
    Ok(value)
}

/// Shared metadata parse for full pages, keys and counts; no malformed rows vanish.
pub fn metadata(value: &serde_json::Value) -> Result<super::traits::ResourceKeyMeta, ConfWriterError> {
    serde::Deserialize::deserialize(&value["metadata"]).map_err(|_| source_error())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceCursor {
    pub version: u8,
    pub backend: String,
    pub generation: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub limit: u32,
    pub position: String,
}
impl SourceCursor {
    pub fn decode(token: &str, scope: &Self) -> Result<Self, ConfWriterError> {
        if token.len() > MAX_CURSOR_BYTES {
            return Err(stale());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| stale())?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| stale())?;
        if cursor.version != scope.version
            || cursor.backend != scope.backend
            || cursor.generation != scope.generation
            || cursor.kind != scope.kind
            || cursor.namespace != scope.namespace
            || cursor.limit != scope.limit
        {
            return Err(stale());
        }
        Ok(cursor)
    }
    pub fn encode(&self) -> Result<String, ConfWriterError> {
        let token = URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).map_err(|_| source_error())?);
        if token.len() > MAX_CURSOR_BYTES {
            return Err(budget_error());
        }
        Ok(token)
    }
}

/// Exact source locators are path components, never user-supplied URLs.
pub fn validate_locator(kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
    let kind = edgion_resources::ResourceKind::from_kind_name(kind)
        .ok_or_else(|| ConfWriterError::ValidationError("Invalid source kind".into()))?;
    let cluster = edgion_resources::resource::is_kind_cluster_scoped(kind);
    if cluster != namespace.is_none()
        || [(name, 253), (namespace.unwrap_or("cluster"), 63)]
            .iter()
            .any(|(value, max)| {
                value.is_empty()
                    || value.len() > *max
                    || value.contains("..")
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
    {
        return Err(ConfWriterError::ValidationError("Invalid source locator".into()));
    }
    Ok(())
}

/// Source equality witness for preview-bound commands; never an authorization token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourcePrecondition {
    pub sha256: String,
    pub uid: Option<String>,
    pub resource_version: Option<String>,
}
impl SourcePrecondition {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, ConfWriterError> {
        use sha2::{Digest, Sha256};
        let mut canonical = value.clone();
        canonical.sort_all_objects();
        let bytes = serde_json::to_vec(&canonical).map_err(|_| source_error())?;
        if bytes.len() > MAX_SOURCE_BYTES {
            return Err(budget_error());
        }
        let field = |name: &str, max: usize| -> Result<Option<String>, ConfWriterError> {
            match value.pointer(name) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(serde_json::Value::String(text)) if text.len() <= max => Ok(Some(text.clone())),
                _ => Err(source_error()),
            }
        };
        Ok(Self {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            uid: field("/metadata/uid", 128)?,
            resource_version: field("/metadata/resourceVersion", 256)?,
        })
    }
    pub fn verify(&self, content: &str) -> Result<serde_json::Value, ConfWriterError> {
        if self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.uid.as_ref().is_some_and(|s| s.len() > 128)
            || self.resource_version.as_ref().is_some_and(|s| s.len() > 256)
        {
            return Err(ConfWriterError::ValidationError("Invalid source precondition".into()));
        }
        if content.len() > MAX_SOURCE_BYTES {
            return Err(budget_error());
        }
        let value: serde_json::Value = serde_yaml::from_str(content).map_err(|_| source_error())?;
        if Self::from_value(&value)? != *self {
            return Err(ConfWriterError::Conflict("Source changed since preview".into()));
        }
        Ok(value)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/source_query_test.rs"]
mod tests;
