//! Bounded snapshot ranges on the existing authenticated Etcd connection.
use super::EtcdLinkClient;
use etcd_client::{GetOptions, SortOrder, SortTarget};
use std::fmt;

pub const SOURCE_RANGE_BYTES: usize = 8 * 1024 * 1024;
pub const SOURCE_RANGE_LIMIT: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRangeError {
    Invalid,
    Expired,
    Denied,
    Budget,
    Timeout,
    Unavailable,
}
impl fmt::Display for SourceRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid source range",
            Self::Expired => "Source revision is unavailable",
            Self::Denied => "Source access denied",
            Self::Budget => "Source range budget exceeded",
            Self::Timeout => "Source range deadline exceeded",
            Self::Unavailable => "Source range unavailable",
        })
    }
}
impl std::error::Error for SourceRangeError {}

pub struct SourceRangePage {
    pub entries: Vec<(String, Vec<u8>, i64)>,
    pub revision: i64,
    pub more: bool,
}

fn range_bounds(prefix: &[u8], after: Option<&[u8]>) -> Result<(Vec<u8>, Vec<u8>), SourceRangeError> {
    if prefix.is_empty() {
        return Err(SourceRangeError::Invalid);
    }
    let mut end = prefix.to_vec();
    while end.last() == Some(&255) {
        end.pop();
    }
    let last = end.last_mut().ok_or(SourceRangeError::Invalid)?;
    *last += 1;
    let start = if let Some(after) = after {
        if !after.starts_with(prefix) {
            return Err(SourceRangeError::Invalid);
        }
        let mut start = after.to_vec();
        start.push(0);
        start
    } else {
        prefix.to_vec()
    };
    if start >= end {
        return Err(SourceRangeError::Invalid);
    }
    Ok((start, end))
}

// The SDK and workspace use different tonic versions. gRPC status numbers are
// stable on the wire; do not add another public transport dependency.
fn range_status(code: i32, message: &str) -> SourceRangeError {
    // tonic 0.14 reports its local frame-size limit as OUT_OF_RANGE too.
    // Inspect only this fixed SDK prefix; never retain the upstream text.
    if code == 11 && message.starts_with("Error, decoded message length too large:") {
        return SourceRangeError::Budget;
    }
    match code {
        11 => SourceRangeError::Expired,    // OUT_OF_RANGE
        7 | 16 => SourceRangeError::Denied, // PERMISSION_DENIED / UNAUTHENTICATED
        8 => SourceRangeError::Budget,      // RESOURCE_EXHAUSTED
        4 => SourceRangeError::Timeout,     // DEADLINE_EXCEEDED
        _ => SourceRangeError::Unavailable,
    }
}
fn range_error(error: etcd_client::Error) -> SourceRangeError {
    match error {
        etcd_client::Error::GRpcStatus(status) => range_status(status.code() as i32, status.message()),
        _ => SourceRangeError::Unavailable,
    }
}

impl EtcdLinkClient {
    /// Exact bounded source read; a missing key never matches a descendant.
    pub async fn get_source_value(&self, key: &str) -> Result<Option<(Vec<u8>, i64)>, SourceRangeError> {
        if key.is_empty() {
            return Err(SourceRangeError::Invalid);
        }
        let full_key = self.full_key(key);
        let client = self.get_client().await.ok_or(SourceRangeError::Unavailable)?;
        let mut kv = client.kv_client().max_decoding_message_size(SOURCE_RANGE_BYTES);
        let response = kv.get(full_key.as_bytes(), None).await.map_err(range_error)?;
        if response.more() || response.kvs().len() > 1 {
            return Err(SourceRangeError::Invalid);
        }
        match response.kvs().first() {
            Some(value) if value.key() == full_key.as_bytes() => {
                Ok(Some((value.value().to_vec(), value.mod_revision())))
            }
            Some(_) => Err(SourceRangeError::Invalid),
            None => Ok(None),
        }
    }

    /// Read one ascending prefix page at a pinned revision. Limits apply to this
    /// KV handle only, leaving Watch, election and other consumers unchanged.
    pub async fn get_prefix_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        revision: Option<i64>,
        limit: u32,
    ) -> Result<SourceRangePage, SourceRangeError> {
        if prefix.is_empty() || limit == 0 || limit > SOURCE_RANGE_LIMIT || revision.is_some_and(|r| r <= 0) {
            return Err(SourceRangeError::Invalid);
        }
        let full_prefix = self.full_key(prefix);
        let full_after = after.map(|key| self.full_key(key));
        let (start, end) = range_bounds(full_prefix.as_bytes(), full_after.as_deref().map(str::as_bytes))?;
        let client = self.get_client().await.ok_or(SourceRangeError::Unavailable)?;
        let mut kv = client.kv_client().max_decoding_message_size(SOURCE_RANGE_BYTES);
        let options = GetOptions::new()
            .with_range(end.clone())
            .with_limit(i64::from(limit))
            .with_revision(revision.unwrap_or(0))
            .with_sort(SortTarget::Key, SortOrder::Ascend);
        let response = kv.get(start.clone(), Some(options)).await.map_err(range_error)?;
        let header = response.header().ok_or(SourceRangeError::Invalid)?;
        if header.revision() <= 0
            || response.kvs().len() > limit as usize
            || (response.more() && response.kvs().is_empty())
        {
            return Err(SourceRangeError::Invalid);
        }
        // A historical read's header may report the newer cluster revision.
        let selected_revision = revision.unwrap_or(header.revision());
        let mut previous: Option<&[u8]> = None;
        let mut entries = Vec::with_capacity(response.kvs().len());
        for value in response.kvs() {
            let key = value.key();
            if !key.starts_with(full_prefix.as_bytes())
                || key < start.as_slice()
                || key >= end.as_slice()
                || previous.is_some_and(|prior| prior >= key)
            {
                return Err(SourceRangeError::Invalid);
            }
            previous = Some(key);
            let key = std::str::from_utf8(key).map_err(|_| SourceRangeError::Invalid)?;
            let logical = match self.namespace() {
                Some(namespace) => key.strip_prefix(namespace).ok_or(SourceRangeError::Invalid)?,
                None => key,
            };
            entries.push((logical.to_owned(), value.value().to_vec(), value.mod_revision()));
        }
        Ok(SourceRangePage {
            entries,
            revision: selected_revision,
            more: response.more(),
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/source_range_test.rs"]
mod tests;
