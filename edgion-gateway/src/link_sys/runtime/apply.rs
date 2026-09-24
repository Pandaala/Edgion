//! Shared apply planning for persistent LinkSys owners.
//!
//! The dispatcher posts desired spec; the per-key runner computes the fingerprint
//! and [`plan_apply`]. Fingerprint bytes are never logged.

use std::fmt;
use std::time::Duration;

use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::TlsConfig;
use k8s_openapi::api::core::v1::Secret;
use sha2::{Digest, Sha256};

// Keep shared TLS policy and consumed Secret material encoding in one place.
// Provider-specific rebuild and credential fields remain with each owner.
pub(crate) fn hash_tls_policy(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    h.tag(u8::from(tls.enabled));
    if !tls.enabled {
        return;
    }
    h.opt_bool(tls.outbound.verify);
    match tls.outbound.validation.as_ref() {
        None => h.tag(0),
        Some(validation) => {
            h.tag(1);
            h.opt_str(validation.hostname.as_deref());
            match validation.well_known_ca_certificates {
                None => h.tag(0),
                Some(WellKnownCACertificates::System) => h.tag(1),
                Some(WellKnownCACertificates::Other(ref name)) => {
                    h.tag(2);
                    h.bytes(name.as_bytes());
                }
            }
            match validation.subject_alt_names.as_deref() {
                None => h.tag(0),
                Some(sans) => {
                    h.tag(1);
                    h.u64(sans.len() as u64);
                    for san in sans {
                        hash_san(h, san);
                    }
                }
            }
            match validation.ca_certificate_refs.as_deref() {
                None => h.tag(0),
                Some(refs) => {
                    h.tag(1);
                    h.u64(refs.len() as u64);
                    for r in refs {
                        h.bytes(r.group.as_bytes());
                        h.bytes(r.kind.as_bytes());
                        h.bytes(r.name.as_bytes());
                        h.opt_str(r.namespace.as_deref());
                    }
                }
            }
        }
    }
    match tls.outbound.client_certificate_ref.as_ref() {
        None => h.tag(0),
        Some(r) => {
            h.tag(1);
            h.opt_str(r.group.as_deref());
            h.opt_str(r.kind.as_deref());
            h.bytes(r.name.as_bytes());
            h.opt_str(r.namespace.as_deref());
        }
    }
}

fn hash_san(h: &mut FingerprintHasher, san: &SubjectAltName) {
    h.tag(match san.san_type {
        SubjectAltNameType::Hostname => 0,
        SubjectAltNameType::URI => 1,
    });
    h.opt_str(san.hostname.as_deref());
    h.opt_str(san.uri.as_deref());
}

pub(crate) fn hash_tls_material(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    if !tls.enabled {
        return;
    }
    hash_secrets(h, tls.resolved_ca_certificates.as_deref());
    hash_secret(h, tls.resolved_client_certificate.as_ref());
}

fn hash_secrets(h: &mut FingerprintHasher, secrets: Option<&[Secret]>) {
    match secrets {
        None => h.tag(0),
        Some(secrets) => {
            h.tag(1);
            h.u64(secrets.len() as u64);
            for secret in secrets {
                hash_secret_data(h, secret);
            }
        }
    }
}

fn hash_secret(h: &mut FingerprintHasher, secret: Option<&Secret>) {
    match secret {
        None => h.tag(0),
        Some(secret) => {
            h.tag(1);
            hash_secret_data(h, secret);
        }
    }
}

fn hash_secret_data(h: &mut FingerprintHasher, secret: &Secret) {
    let mut entries: Vec<(&String, &k8s_openapi::ByteString)> = secret
        .data
        .as_ref()
        .map(|data| data.iter().collect())
        .unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    h.u64(entries.len() as u64);
    for (key, value) in entries {
        h.bytes(key.as_bytes());
        h.bytes(&value.0);
    }
}

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

/// Length-prefixed SHA-256 hasher for rebuild / secret fingerprints.
pub struct FingerprintHasher(Sha256);

impl FingerprintHasher {
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    pub fn tag(&mut self, tag: u8) {
        self.0.update([tag]);
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value);
    }

    pub fn u64(&mut self, value: u64) {
        self.0.update(value.to_le_bytes());
    }

    pub fn opt_str(&mut self, value: Option<&str>) {
        match value {
            None => self.tag(0),
            Some(s) => {
                self.tag(1);
                self.bytes(s.as_bytes());
            }
        }
    }

    pub fn opt_bool(&mut self, value: Option<bool>) {
        match value {
            None => self.tag(0),
            Some(false) => self.tag(1),
            Some(true) => self.tag(2),
        }
    }

    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl Default for FingerprintHasher {
    fn default() -> Self {
        Self::new()
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
