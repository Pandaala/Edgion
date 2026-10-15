//! Shared Controller↔Gateway conf-sync link settings.
//!
//! These types describe the conf-sync gRPC channel contract (capacities,
//! no-sync kinds, and mTLS security). Both planes need them: the Controller
//! side (conf-sync server, federation client) and the Gateway side (conf-sync
//! client), plus `grpc_tls` which loads the certificates. They live in `common`
//! as the single shared definition of the link contract.

use crate::conf_sync::cert_duration::CertDuration;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Default cache capacity for ResourceProcessor EventStore
fn default_cache_capacity() -> u32 {
    200
}

/// Configuration synchronization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ConfSyncConfig {
    /// Default EventStore capacity for all resource types
    /// This is used for all resource types unless a specific override is configured
    #[arg(skip)]
    #[serde(default = "default_cache_capacity")]
    pub default_capacity: u32,

    /// Override capacity for specific resource kinds
    /// Key: resource kind name (e.g., "GatewayClass", "HTTPRoute")
    /// Value: capacity for that resource type
    #[arg(skip)]
    #[serde(default)]
    pub capacity_overrides: Option<HashMap<String, u32>>,

    /// Resource kinds that should NOT be synced to Gateway.
    /// If not configured, uses DEFAULT_NO_SYNC_KINDS from types module.
    /// When configured, completely overrides the default list.
    #[arg(skip)]
    #[serde(default)]
    pub no_sync_kinds: Option<Vec<String>>,
}

impl ConfSyncConfig {
    /// Get the list of resource kinds that should not be synced to Gateway.
    /// Returns configured list if set, otherwise returns DEFAULT_NO_SYNC_KINDS.
    pub fn get_no_sync_kinds(&self) -> Vec<&str> {
        match &self.no_sync_kinds {
            Some(kinds) => kinds.iter().map(|s| s.as_str()).collect(),
            None => edgion_resources::DEFAULT_NO_SYNC_KINDS.to_vec(),
        }
    }

    /// Get the capacity for a specific resource kind.
    ///
    /// Resolution order:
    /// 1. `capacity_overrides` map (explicit per-resource override from config)
    /// 2. `default_capacity` (global default, default 200)
    pub fn get_capacity(&self, kind_name: &str) -> u32 {
        if let Some(v) = self
            .capacity_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(kind_name))
        {
            return *v;
        }
        self.default_capacity
    }
}

impl Default for ConfSyncConfig {
    fn default() -> Self {
        Self {
            default_capacity: default_cache_capacity(),
            capacity_overrides: None,
            no_sync_kinds: None, // Use DEFAULT_NO_SYNC_KINDS
        }
    }
}

/// conf_sync gRPC channel security configuration.
/// Both planes require an explicit transport choice; an omitted section fails closed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConfSyncSecurityConfig {
    /// Name of the active certificate set. Must match `name` of one entry in `certs`.
    pub active: Option<String>,
    /// Certificate sets (YAML `conf_sync_security.certs` list).
    pub certs: Vec<ConfSyncTlsConfig>,
    /// When true (Kubernetes only), the Controller's internal CA auto-provisions the
    /// conf_sync mTLS certs (CA + gateway-client Secret) instead of requiring
    /// manually-mounted cert files. Default false (manual file certs).
    #[serde(default)]
    pub auto_ca: bool,
    /// Rotation policy for the auto-CA (controller-only; the gateway ignores this).
    #[serde(default)]
    pub rotation: Option<ConfSyncRotationConfig>,
    // Reserved: token authentication can be added here later
}

impl ConfSyncSecurityConfig {
    /// Resolve the active TLS config.
    ///
    /// Priority:
    /// 1. `certs` array: find entry where `name == active`
    /// 2. `certs` has exactly one entry and `active` is None: use it
    /// 3. `None` = no active file-based TLS config
    ///
    /// Callers must validate the selection and enforce the transport configuration.
    /// Plaintext requires an explicit `skip_tls` on the selected config.
    pub fn resolve_active_tls(&self) -> Option<&ConfSyncTlsConfig> {
        if !self.certs.is_empty() {
            if let Some(active) = &self.active {
                return self.certs.iter().find(|c| c.name.as_deref() == Some(active.as_str()));
            }
            if self.certs.len() == 1 {
                return self.certs.first();
            }
            return None;
        }
        None
    }

    /// True when the security section is entirely unconfigured: no cert sets,
    /// and no `active` selector. Since 2026-07-14 this state is
    /// FATAL for conf_sync on both planes — see [`Self::ensure_transport_configured`].
    ///
    /// Distinct from `skip_tls` (certs present but bypassed — still the explicit,
    /// supported way to run plaintext) and from a dangling `active` selector
    /// (rejected by `validate()`).
    pub fn is_plaintext_omitted(&self) -> bool {
        self.certs.is_empty() && self.active.is_none()
    }

    /// Fail-close guard for the conf_sync transport (Controller↔Gateway).
    ///
    /// The conf_sync channel streams **resolved Secrets** (TLS private keys,
    /// backend credentials) to the data plane. An omitted `conf_sync_security`
    /// therefore must never mean "run in plaintext": plaintext has to be an
    /// explicit, audited choice (`skip_tls: true`), never the consequence of a
    /// forgotten config block. Both the Controller (conf_sync gRPC server) and
    /// the Gateway (conf_sync gRPC client) call this at startup and refuse to
    /// start when it errors.
    ///
    /// `auto_ca: true` is itself a complete transport choice (the shipped Kubernetes
    /// default): the Controller mints its serving identity in memory from its internal
    /// CA, so it legitimately carries no cert paths and is NOT the omitted state here.
    /// The gateway separately requires cert paths under `auto_ca` and fails closed on
    /// its own (`ConfigSyncClient::new`).
    ///
    /// Deliberately NOT folded into [`Self::validate`]: `validate` is also run
    /// by consumers that only parse the block (e.g. the gateway parsing the
    /// controller-only `rotation` sub-block) and must stay lenient about an
    /// entirely-empty section.
    pub fn ensure_transport_configured(&self) -> anyhow::Result<()> {
        if self.is_plaintext_omitted() && !self.auto_ca {
            anyhow::bail!(
                "conf_sync_security is not configured: refusing to start. The conf_sync channel \
                 carries resolved Secrets (TLS private keys, backend credentials) to the data \
                 plane, so plaintext must be an explicit, audited choice — never the result of an \
                 omitted config block. Choose one: (a) set `conf_sync_security.auto_ca: true` \
                 (Kubernetes: the Controller's internal CA provisions the CA and the gateway \
                 client cert automatically); (b) configure `conf_sync_security.certs` with \
                 `cert`/`key`/`ca` paths for file-mode \
                 mTLS; (c) if you genuinely intend plaintext, set `skip_tls: true` on the active \
                 cert set to opt in explicitly."
            );
        }
        Ok(())
    }

    /// Federation requires real mTLS. Returns the active cert set only when one
    /// is configured AND `skip_tls` is not set; otherwise refuses with a reason.
    /// Plaintext is never representable through this path.
    ///
    /// Precondition: assumes `validate()` has already passed — it does NOT
    /// re-check cert path emptiness. conf_sync does NOT use this; it keeps
    /// `resolve_active_tls()` + `skip_tls`.
    pub fn resolve_mtls_or_refuse(&self) -> Result<&ConfSyncTlsConfig, MtlsRefusal> {
        match self.resolve_active_tls() {
            None => Err(MtlsRefusal::NoTls),
            Some(t) if t.skip_tls => Err(MtlsRefusal::SkipTls),
            Some(t) => Ok(t),
        }
    }

    /// Validate configuration at startup.
    ///
    /// Returns error for:
    /// - duplicate names in `certs`
    /// - multiple certs without `active`
    /// - `active` pointing to nonexistent name
    /// - cert path fields empty on active cert (unless `skip_tls`)
    pub fn validate(&self) -> anyhow::Result<()> {
        // Check duplicate names
        let mut seen = std::collections::HashSet::new();
        for cert in &self.certs {
            if let Some(name) = &cert.name {
                if !seen.insert(name.as_str()) {
                    anyhow::bail!("conf_sync_security: duplicate cert name '{}'", name);
                }
            }
        }
        // Multiple certs require active selector
        if self.certs.len() > 1 && self.active.is_none() {
            anyhow::bail!("conf_sync_security: multiple certs configured but no 'active' selector");
        }
        // active must match an existing name
        if let Some(active) = &self.active {
            if !self.certs.is_empty() {
                let found = self.certs.iter().any(|c| c.name.as_deref() == Some(active.as_str()));
                if !found {
                    anyhow::bail!("conf_sync_security: active='{}' does not match any cert name", active);
                }
            }
        }
        // Reject a dangling `active` selector: it names a cert set but no certs
        // are configured. Without this guard the config
        // passes validation yet `resolve_active_tls()` returns None, which would
        // be indistinguishable from a truly-omitted section at the channel builder.
        if self.active.is_some() && self.certs.is_empty() {
            anyhow::bail!("conf_sync_security: 'active' is set but no certs are configured");
        }
        // Validate the active cert's paths (skip if skip_tls)
        if let Some(tls) = self.resolve_active_tls() {
            if !tls.skip_tls {
                tls.validate()?;
            }
        }
        Ok(())
    }
}

/// mTLS certificate file paths for conf_sync gRPC channel.
#[derive(Clone, Serialize, Deserialize)]
pub struct ConfSyncTlsConfig {
    /// Name for this cert set. Required for selection with `active`.
    #[serde(default)]
    pub name: Option<String>,
    /// Certificate file path (relative to work_dir or absolute).
    /// Optional ONLY so the `skip_tls: true` escape hatch can be written without
    /// dummy paths; `validate()` still rejects an empty path on an active cert set
    /// that is not `skip_tls`.
    #[serde(default)]
    pub cert: String,
    /// Private key file path (see `cert` for why this is `#[serde(default)]`).
    #[serde(default)]
    pub key: String,
    /// CA certificate path, used to verify the peer (see `cert`).
    #[serde(default)]
    pub ca: String,
    /// Explicit plaintext opt-in: skip TLS and start in plaintext with an ERROR log
    /// and the `skip_tls` system-error metric. This is the ONLY way to run the
    /// Secret-carrying conf_sync channel in plaintext — an omitted `conf_sync_security`
    /// refuses to start. Emergency / local-dev use only; must not stay enabled.
    #[serde(default)]
    pub skip_tls: bool,
}

impl std::fmt::Debug for ConfSyncTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfSyncTlsConfig")
            .field("name", &self.name)
            .field("cert", &self.cert)
            .field("key", &"***")
            .field("ca", &self.ca)
            .field("skip_tls", &self.skip_tls)
            .finish()
    }
}

impl ConfSyncTlsConfig {
    /// Validate that all required paths are non-empty.
    /// Called before attempting to load certificates.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cert.trim().is_empty() {
            anyhow::bail!("conf_sync_security.certs[].cert must not be empty");
        }
        if self.key.trim().is_empty() {
            anyhow::bail!("conf_sync_security.certs[].key must not be empty");
        }
        if self.ca.trim().is_empty() {
            anyhow::bail!("conf_sync_security.certs[].ca must not be empty");
        }
        Ok(())
    }

    /// Resolve a path: absolute paths used as-is, relative paths joined with work_dir.
    fn resolve_path(path: &str) -> PathBuf {
        edgion_resources::work_dir().resolve(path)
    }

    pub fn cert_path(&self) -> PathBuf {
        Self::resolve_path(&self.cert)
    }

    pub fn key_path(&self) -> PathBuf {
        Self::resolve_path(&self.key)
    }

    pub fn ca_path(&self) -> PathBuf {
        Self::resolve_path(&self.ca)
    }
}

/// Why a federation channel refused the configured security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtlsRefusal {
    /// resolve_active_tls() == None — no TLS block configured.
    NoTls,
    /// The active cert set has skip_tls = true.
    SkipTls,
}

/// Leaf-rotation policy for the conf_sync auto-CA (spec §4). Controller-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RotationPolicy {
    #[default]
    Lifetime,
    Scheduled,
    Manual,
}

/// Rotation configuration (spec §5.1). Parsed by both binaries; CONSUMED BY THE
/// CONTROLLER ONLY — the gateway ignores it (and must never fail startup on it).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfSyncRotationConfig {
    pub policy: RotationPolicy,
    pub lifetime_ratio: f64,
    /// 5-field crontab; required iff policy == scheduled. Cron/timezone parsing is
    /// validated controller-side only (croner is a controller dependency).
    pub schedule: Option<String>,
    pub schedule_timezone: String,
    pub rotate_if_remaining_below: CertDuration,
    pub emergency_rotate_below: CertDuration,
    pub leaf_ttl: CertDuration,
    pub ca_ttl: CertDuration,
}

impl Default for ConfSyncRotationConfig {
    fn default() -> Self {
        Self {
            policy: RotationPolicy::Lifetime,
            lifetime_ratio: 0.5,
            schedule: None,
            schedule_timezone: "UTC".to_string(),
            rotate_if_remaining_below: CertDuration::from_hours(336),
            emergency_rotate_below: CertDuration::from_hours(24),
            leaf_ttl: CertDuration::from_hours(365 * 24),
            ca_ttl: CertDuration::from_hours(3650 * 24),
        }
    }
}

impl ConfSyncRotationConfig {
    /// Numeric invariants (spec §5.2). Cron parseability / timezone resolution /
    /// firing-gap feasibility are validated controller-side (needs croner).
    pub fn validate_numeric(&self) -> anyhow::Result<()> {
        let leaf = self.leaf_ttl.as_std();
        let ca = self.ca_ttl.as_std();
        let guard = self.rotate_if_remaining_below.as_std();
        let emergency = self.emergency_rotate_below.as_std();
        anyhow::ensure!(leaf >= std::time::Duration::from_secs(3600), "leaf_ttl must be >= 1h");
        anyhow::ensure!(ca >= std::time::Duration::from_secs(24 * 3600), "ca_ttl must be >= 1d");
        anyhow::ensure!(
            leaf < ca,
            "leaf_ttl ({}) must be < ca_ttl ({})",
            self.leaf_ttl,
            self.ca_ttl
        );
        anyhow::ensure!(
            emergency < leaf,
            "emergency_rotate_below ({}) must be < leaf_ttl ({})",
            self.emergency_rotate_below,
            self.leaf_ttl
        );
        match self.policy {
            RotationPolicy::Scheduled => {
                anyhow::ensure!(self.schedule.is_some(), "policy=scheduled requires `schedule`");
                anyhow::ensure!(
                    emergency < guard,
                    "emergency_rotate_below ({}) must be < rotate_if_remaining_below ({})",
                    self.emergency_rotate_below,
                    self.rotate_if_remaining_below
                );
                // Churn hole (spec §5.2): guard capped at half the leaf lifetime.
                anyhow::ensure!(
                    guard.as_secs() * 2 <= leaf.as_secs(),
                    "rotate_if_remaining_below ({}) must be <= 0.5 x leaf_ttl ({})",
                    self.rotate_if_remaining_below,
                    self.leaf_ttl
                );
            }
            RotationPolicy::Lifetime => {
                anyhow::ensure!(
                    self.lifetime_ratio > 0.0 && self.lifetime_ratio < 1.0,
                    "lifetime_ratio must be in (0.0, 1.0) exclusive"
                );
                // The emergency net must not always preempt the lifetime trigger.
                let post_trigger = leaf.as_secs_f64() * (1.0 - self.lifetime_ratio);
                anyhow::ensure!(
                    (emergency.as_secs() as f64) < post_trigger,
                    "emergency_rotate_below ({}) must be < (1 - lifetime_ratio) x leaf_ttl",
                    self.emergency_rotate_below
                );
            }
            RotationPolicy::Manual => {}
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/conf_sync/config_test.rs"]
mod tests;
