//! Rule values a delegated leaf inherits from its ancestors.
//!
//! `timeouts` and `retry` fall back per sub-field: the deepest level that sets
//! a sub-field wins. `sessionPersistence` is taken as a whole block from the
//! deepest level that sets it. A subtree is shared by every parent
//! that delegates to it, so the fallback cannot be precomputed per unit. It is
//! folded once at match time over the matched ancestors (root -> parent) and
//! stored in `DelegatedMatch`. Context accessors read the leaf value first and
//! consult this only when the leaf leaves the sub-field unset; ordinary routes
//! never build or read it.

use crate::routes::grpc::GrpcRouteRuleUnit;
use crate::routes::http::HttpRouteRuleUnit;
use edgion_resources::resources::http_route::{ParsedRouteRetry, ParsedRouteTimeouts, RouteTimeout};
use std::sync::Arc;
use std::time::Duration;

/// Ancestor-provided fallbacks for the leaf's unset rule sub-fields.
///
/// Scalar sub-fields are copied. Borrowed sub-fields (`retry.codes`, the
/// `sessionPersistence` block) are recorded as an index into
/// `DelegatedMatch::ancestors`, so folding never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InheritedRuleValues {
    /// Nearest ancestor `timeouts.request` that is set (`Disabled` counts as set).
    pub request_timeout: Option<RouteTimeout>,
    /// Nearest ancestor `timeouts.backendRequest` that is set.
    pub backend_request_timeout: Option<RouteTimeout>,
    /// Nearest ancestor `retry.attempts`.
    pub retry_attempts: Option<u32>,
    /// Nearest ancestor `retry.backoff`.
    pub retry_backoff: Option<Duration>,
    /// Ancestor index holding the nearest `retry.codes`.
    pub retry_codes_level: Option<u8>,
    /// Whether any ancestor declares a retry policy.
    pub retry_configured: bool,
    /// Ancestor index holding the nearest `sessionPersistence` block. The
    /// block is taken whole, matching the existing block-level precedence.
    pub session_persistence_level: Option<u8>,
}

impl InheritedRuleValues {
    /// Fold HTTPRoute ancestors root -> parent; a deeper level overrides per sub-field.
    pub fn fold(ancestors: &[Arc<HttpRouteRuleUnit>]) -> Self {
        Self::fold_rules(ancestors.iter().map(|unit| {
            let rule = &unit.rule;
            (
                rule.parsed_timeouts.as_ref(),
                rule.parsed_retry.as_ref(),
                rule.session_persistence.is_some(),
            )
        }))
    }

    /// Fold GRPCRoute ancestors root -> parent with the same per-sub-field rules.
    pub fn fold_grpc(ancestors: &[Arc<GrpcRouteRuleUnit>]) -> Self {
        Self::fold_rules(ancestors.iter().map(|unit| {
            let rule = &unit.rule;
            (
                rule.parsed_timeouts.as_ref(),
                rule.parsed_retry.as_ref(),
                rule.session_persistence.is_some(),
            )
        }))
    }

    /// Shared fold over `(timeouts, retry, has sessionPersistence)` per level.
    fn fold_rules<'a>(
        levels: impl Iterator<Item = (Option<&'a ParsedRouteTimeouts>, Option<&'a ParsedRouteRetry>, bool)>,
    ) -> Self {
        let mut out = Self::default();
        for (index, (timeouts, retry, has_session_persistence)) in levels.enumerate() {
            if let Some(timeouts) = timeouts {
                if timeouts.request_timeout != RouteTimeout::Unset {
                    out.request_timeout = Some(timeouts.request_timeout);
                }
                if timeouts.backend_request_timeout != RouteTimeout::Unset {
                    out.backend_request_timeout = Some(timeouts.backend_request_timeout);
                }
            }
            if let Some(retry) = retry {
                out.retry_configured = true;
                if retry.attempts.is_some() {
                    out.retry_attempts = retry.attempts;
                }
                if retry.backoff.is_some() {
                    out.retry_backoff = retry.backoff;
                }
                if retry.codes.is_some() {
                    out.retry_codes_level = Some(index as u8);
                }
            }
            if has_session_persistence {
                out.session_persistence_level = Some(index as u8);
            }
        }
        out
    }
}

/// A route timeout sub-field as a leaf-first lookup: `None` means "not set at
/// this level", so the caller falls back to the inherited value.
#[inline]
pub(crate) fn set_timeout(value: RouteTimeout) -> Option<RouteTimeout> {
    (value != RouteTimeout::Unset).then_some(value)
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/delegation/inherit_test.rs"]
mod tests;
