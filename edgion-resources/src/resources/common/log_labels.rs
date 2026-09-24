//! Operator-defined log labels resolved from resource metadata.
//!
//! One annotation carrier feeds the TLS log, resolved here:
//! - [`LOG_LABEL_PREFIX`](crate::constants::annotations::edgion::LOG_LABEL_PREFIX)
//!   (`edgion.io/log-label.<suffix>`) — contributes `<suffix> = <value>` to the
//!   resource's `rsc.labels` (value given inline).
//!
//! This carrier is the sole surviving mechanism for TLSRoute and EdgionTls,
//! whose handshake layer has no plugin chain and no `logging()` stage to hook
//! an `AccessLogExtern` declaration onto. HTTPRoute/GRPCRoute log projection is
//! declared via `AccessLogExtern` instead.
//!
//! Resolution happens **once, at resource-parse time** (route conf-handler build
//! or `ResourceMeta::pre_parse`), never on the per-request / per-handshake data
//! path. The result is an `Arc<..>` so it can be shared by reference (cheap
//! clone, deterministic ordering) into every log entry.

use std::collections::BTreeMap;
use std::sync::Arc;

use kube::api::ObjectMeta;

use crate::constants::annotations::edgion::LOG_LABEL_PREFIX;

/// Upper bounds for operator-defined log labels (guardrails against log bloat
/// and high downstream cardinality). Over-limit entries are dropped, not errored.
pub const LOG_LABEL_MAX_COUNT: usize = 16;
pub const LOG_LABEL_MAX_KEY_LEN: usize = 63;
pub const LOG_LABEL_MAX_VALUE_LEN: usize = 256;

/// Resolve operator-defined log labels from a resource's `metadata`.
///
/// `annotations` and `labels` keys prefixed with `edgion.io/log-label.`
/// contribute `<suffix> = <value>` to `rsc.labels`; the suffix after the prefix
/// becomes the label key (`labels` first, then `annotations` override on
/// collision). The value is given inline by the prefixed entry.
///
/// Entries whose key/value exceed the length caps are dropped, and the result is
/// capped at [`LOG_LABEL_MAX_COUNT`]. Returns `None` when nothing matched, so the
/// field is omitted from the log entry. The returned map is wrapped in an `Arc`
/// so the resolved set can be shared by reference across log entries.
pub fn resolve_log_labels(meta: &ObjectMeta) -> Option<Arc<BTreeMap<String, String>>> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();

    // `edgion.io/log-label.<suffix>` prefix: labels first, annotations override.
    for src in [meta.labels.as_ref(), meta.annotations.as_ref()] {
        let Some(map) = src else { continue };
        for (key, value) in map {
            let Some(suffix) = key.strip_prefix(LOG_LABEL_PREFIX) else {
                continue;
            };
            if suffix.is_empty() || suffix.len() > LOG_LABEL_MAX_KEY_LEN || value.len() > LOG_LABEL_MAX_VALUE_LEN {
                continue;
            }
            out.insert(suffix.to_string(), value.clone());
        }
    }

    if out.is_empty() {
        return None;
    }
    if out.len() > LOG_LABEL_MAX_COUNT {
        // BTreeMap iterates in sorted key order, so the cap is deterministic.
        let capped: BTreeMap<String, String> = out.into_iter().take(LOG_LABEL_MAX_COUNT).collect();
        return Some(Arc::new(capped));
    }
    Some(Arc::new(out))
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/log_labels_test.rs"]
mod tests;
