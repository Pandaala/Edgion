//! Shared apply planning for persistent LinkSys owners.
//!
//! The dispatcher posts desired spec; the per-key runner computes the fingerprint
//! and [`plan_apply`]. Fingerprint bytes are never logged.

use std::fmt;
use std::time::Duration;

pub(crate) use edgion_common::link_sys_core::fingerprint::{hash_tls_material, hash_tls_policy, FingerprintHasher};

/// Runner connect backoff: 2s ×2, cap 30s. Distinct from health-probe backoff.
pub const CONNECT_BACKOFF_START: Duration = Duration::from_secs(2);
pub const CONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);
pub const HANDOVER_RETIRE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPlan {
    Reuse,
    ReplaceNow,
    Handover,
    /// Executed by cancelling the runner, not by [`plan_apply`].
    #[allow(dead_code)]
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionCoverage {
    pub live: usize,
    pub configured: usize,
}

/// Hash of rebuild fields + consumed secret material + last-applied RV.
///
/// Debug/Display redact the hashes. Do not log this type.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeFingerprint {
    rebuild: [u8; 32],
    secret: [u8; 32],
    resource_version: Option<String>,
}

impl RuntimeFingerprint {
    pub fn from_parts(rebuild: [u8; 32], secret: [u8; 32], resource_version: Option<String>) -> Self {
        Self {
            rebuild,
            secret,
            resource_version,
        }
    }

    /// Placeholder for clients built outside the runner (unit tests of `from_config`).
    pub fn unspecified() -> Self {
        Self::from_parts([0; 32], [0; 32], None)
    }

    pub fn is_unspecified(&self) -> bool {
        self.rebuild == [0; 32] && self.secret == [0; 32]
    }

    pub fn equal_runtime(&self, other: &Self) -> bool {
        if self.is_unspecified() || other.is_unspecified() {
            return false;
        }
        self.rebuild == other.rebuild && self.secret == other.secret
    }

    /// K8s resourceVersion skip: same RV + same secret hash → Reuse without
    /// treating rebuild-hash inequality as a generation change.
    pub fn rv_skip_allows_reuse(&self, other: &Self) -> bool {
        matches!(
            (&self.resource_version, &other.resource_version),
            (Some(live), Some(incoming)) if live == incoming && self.secret == other.secret
        )
    }

    pub(crate) fn into_parts(self) -> ([u8; 32], [u8; 32], Option<String>) {
        (self.rebuild, self.secret, self.resource_version)
    }
}

impl fmt::Debug for RuntimeFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeFingerprint")
            .field("rebuild", &"<redacted>")
            .field("secret", &"<redacted>")
            .field("resource_version", &self.resource_version)
            .finish()
    }
}

impl fmt::Display for RuntimeFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeFingerprint(<redacted>)")
    }
}

/// Default apply decision. `old is None` is not an [`ApplyPlan`]: the runner
/// connect loop handles first publish.
pub fn plan_apply(
    live: Option<&RuntimeFingerprint>,
    incoming: &RuntimeFingerprint,
    coverage: ConnectionCoverage,
    forced: bool,
) -> ApplyPlan {
    if !forced {
        if let Some(live) = live {
            if !live.is_unspecified()
                && !incoming.is_unspecified()
                && (live.equal_runtime(incoming) || live.rv_skip_allows_reuse(incoming))
            {
                return ApplyPlan::Reuse;
            }
        }
    }
    if coverage.live == 0 {
        ApplyPlan::ReplaceNow
    } else {
        ApplyPlan::Handover
    }
}

pub fn next_connect_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys/runtime/apply_test.rs"]
mod tests;
