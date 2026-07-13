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
/// The entire section is optional; omitting it = no security enhancement (backward compatible).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConfSyncSecurityConfig {
    /// Name of the active certificate set. Must match `name` of one entry in `certs`.
    pub active: Option<String>,
    /// Certificate sets (YAML `conf_sync_security.certs` list).
    pub certs: Vec<ConfSyncTlsConfig>,
    /// Legacy single-cert format [conf_sync_security.tls] — kept for backward compat.
    pub tls: Option<ConfSyncTlsConfig>,
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
    /// 3. Fallback to legacy `tls` field
    /// 4. `None` = plaintext gRPC
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
        self.tls.as_ref()
    }

    /// True when the security section is entirely unconfigured: no cert sets,
    /// no legacy `tls`, and no `active` selector. In this state conf_sync runs
    /// in plaintext and the channel builder marks `PLAINTEXT_DEFAULT`.
    ///
    /// Distinct from `skip_tls` (certs present but bypassed) and from a dangling
    /// `active` selector (rejected by `validate()`).
    pub fn is_plaintext_omitted(&self) -> bool {
        self.certs.is_empty() && self.tls.is_none() && self.active.is_none()
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
        // (and no legacy `tls`) are configured. Without this guard the config
        // passes validation yet `resolve_active_tls()` returns None, which would
        // be indistinguishable from a truly-omitted section at the channel builder.
        if self.active.is_some() && self.certs.is_empty() && self.tls.is_none() {
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
    /// Unique name for this cert set. Required when using [[certs]] array format.
    #[serde(default)]
    pub name: Option<String>,
    /// Certificate file path (relative to work_dir or absolute)
    pub cert: String,
    /// Private key file path
    pub key: String,
    /// CA certificate path (used to verify the peer)
    pub ca: String,
    /// Emergency bypass: skip TLS and start in plaintext with ERROR log.
    /// Only for emergency use during cert rotation failures. Must not stay enabled.
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
            anyhow::bail!("conf_sync_security.tls.cert must not be empty");
        }
        if self.key.trim().is_empty() {
            anyhow::bail!("conf_sync_security.tls.key must not be empty");
        }
        if self.ca.trim().is_empty() {
            anyhow::bail!("conf_sync_security.tls.ca must not be empty");
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
mod tests {
    use super::*;

    // ── Helper ───────────────────────────────────────────────────────────────

    fn make_tls(name: Option<&str>, cert: &str) -> ConfSyncTlsConfig {
        ConfSyncTlsConfig {
            name: name.map(|s| s.to_string()),
            cert: cert.to_string(),
            key: "k".to_string(),
            ca: "ca".to_string(),
            skip_tls: false,
        }
    }

    // ── Group A: get_capacity (3 → 1 table-driven) ──────────────────────────

    #[test]
    fn test_get_capacity_table_driven() {
        struct Case {
            name: &'static str,
            default_capacity: u32,
            override_entry: Option<(&'static str, u32)>, // (kind, cap) to insert
            query_kind: &'static str,
            expected: u32,
        }

        let cases = [
            Case {
                name: "override wins for known kind",
                default_capacity: 200,
                override_entry: Some(("HTTPRoute", 999)),
                query_kind: "HTTPRoute",
                expected: 999,
            },
            Case {
                name: "falls back to default when no overrides",
                default_capacity: 200,
                override_entry: None,
                query_kind: "GatewayClass",
                expected: 200,
            },
            Case {
                name: "unknown kind falls back to global default",
                default_capacity: 200,
                override_entry: None,
                query_kind: "SomeUnknownKind",
                expected: 200,
            },
        ];

        for case in &cases {
            let capacity_overrides = case.override_entry.map(|(kind, cap)| {
                let mut m = HashMap::new();
                m.insert(kind.to_string(), cap);
                m
            });
            let config = ConfSyncConfig {
                default_capacity: case.default_capacity,
                capacity_overrides,
                no_sync_kinds: None,
            };
            assert_eq!(
                config.get_capacity(case.query_kind),
                case.expected,
                "case: {}",
                case.name
            );
        }
    }

    #[test]
    fn test_conf_sync_default() {
        let config = ConfSyncConfig::default();
        assert_eq!(config.default_capacity, 200);
        assert!(config.capacity_overrides.is_none());
    }

    #[test]
    fn auto_ca_defaults_false_and_parses() {
        assert!(!ConfSyncSecurityConfig::default().auto_ca);
        let parsed: ConfSyncSecurityConfig = serde_json::from_str(r#"{"auto_ca": true}"#).unwrap();
        assert!(parsed.auto_ca);
        // Omitting the field keeps the backward-compatible default.
        let omitted: ConfSyncSecurityConfig = serde_json::from_str("{}").unwrap();
        assert!(!omitted.auto_ca);
    }

    // ── Group C: resolve_active_tls (5 → 1 table-driven) ───────────────────

    #[test]
    fn test_resolve_active_tls_scenarios() {
        struct Case {
            name: &'static str,
            cfg: ConfSyncSecurityConfig,
            expected_cert: Option<&'static str>,
        }

        let cases = [
            Case {
                name: "legacy tls field falls back",
                cfg: ConfSyncSecurityConfig {
                    tls: Some(make_tls(None, "legacy.crt")),
                    ..Default::default()
                },
                expected_cert: Some("legacy.crt"),
            },
            Case {
                name: "single cert entry with no active selector",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("primary"), "primary.crt")],
                    ..Default::default()
                },
                expected_cert: Some("primary.crt"),
            },
            Case {
                name: "multi certs with matching active selector",
                cfg: ConfSyncSecurityConfig {
                    active: Some("new".to_string()),
                    certs: vec![make_tls(Some("old"), "old.crt"), make_tls(Some("new"), "new.crt")],
                    ..Default::default()
                },
                expected_cert: Some("new.crt"),
            },
            Case {
                name: "multi certs without active selector returns None",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                    ..Default::default()
                },
                expected_cert: None,
            },
            Case {
                name: "empty config returns None",
                cfg: ConfSyncSecurityConfig::default(),
                expected_cert: None,
            },
        ];

        for case in &cases {
            assert_eq!(
                case.cfg.resolve_active_tls().map(|t| t.cert.as_str()),
                case.expected_cert,
                "case: {}",
                case.name
            );
        }
    }

    // ── Group D: validate_active_tls (5 → 1 table-driven) ──────────────────

    #[test]
    fn test_validate_active_tls_scenarios() {
        enum Expected {
            Ok,
            Err(&'static str),
        }

        struct Case {
            name: &'static str,
            cfg: ConfSyncSecurityConfig,
            expected: Expected,
        }

        let cases = [
            Case {
                name: "empty config is valid",
                cfg: ConfSyncSecurityConfig::default(),
                expected: Expected::Ok,
            },
            Case {
                name: "single entry with skip_tls is valid",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![ConfSyncTlsConfig {
                        name: Some("p".to_string()),
                        cert: String::new(),
                        key: String::new(),
                        ca: String::new(),
                        skip_tls: true,
                    }],
                    ..Default::default()
                },
                expected: Expected::Ok,
            },
            Case {
                name: "duplicate cert names errors",
                cfg: ConfSyncSecurityConfig {
                    active: Some("dup".to_string()),
                    certs: vec![make_tls(Some("dup"), "a.crt"), make_tls(Some("dup"), "b.crt")],
                    ..Default::default()
                },
                expected: Expected::Err(""),
            },
            Case {
                name: "multi certs without active selector errors",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                    ..Default::default()
                },
                expected: Expected::Err("no 'active' selector"),
            },
            Case {
                name: "active selector not found in certs errors",
                cfg: ConfSyncSecurityConfig {
                    active: Some("missing".to_string()),
                    certs: vec![make_tls(Some("primary"), "primary.crt")],
                    ..Default::default()
                },
                expected: Expected::Err("does not match any cert name"),
            },
        ];

        for case in &cases {
            let result = case.cfg.validate();
            match &case.expected {
                Expected::Ok => {
                    assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
                }
                Expected::Err(substr) => {
                    assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                    if !substr.is_empty() {
                        let msg = format!("{:?}", result.unwrap_err());
                        assert!(
                            msg.contains(substr),
                            "case '{}': error message {:?} does not contain {:?}",
                            case.name,
                            msg,
                            substr
                        );
                    }
                }
            }
        }
    }

    // ── Group G: resolve_mtls_or_refuse ─────────────────────────────────────

    #[test]
    fn resolve_mtls_or_refuse_no_tls() {
        let sec = ConfSyncSecurityConfig::default();
        assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::NoTls);
    }

    #[test]
    fn resolve_mtls_or_refuse_skip_tls() {
        let sec = ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: true,
            }),
            ..Default::default()
        };
        assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::SkipTls);
    }

    #[test]
    fn resolve_mtls_or_refuse_ok_for_mtls() {
        let sec = ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: false,
            }),
            ..Default::default()
        };
        assert!(sec.resolve_mtls_or_refuse().is_ok());
    }

    #[test]
    fn is_plaintext_omitted_scenarios() {
        // Truly empty section → omitted.
        assert!(ConfSyncSecurityConfig::default().is_plaintext_omitted());

        // Legacy tls present → not omitted.
        let with_tls = ConfSyncSecurityConfig {
            tls: Some(make_tls(None, "c.crt")),
            ..Default::default()
        };
        assert!(!with_tls.is_plaintext_omitted());

        // certs present → not omitted.
        let with_certs = ConfSyncSecurityConfig {
            certs: vec![make_tls(Some("primary"), "p.crt")],
            ..Default::default()
        };
        assert!(!with_certs.is_plaintext_omitted());

        // active set but nothing else → NOT omitted (it is a dangling misconfig).
        let dangling = ConfSyncSecurityConfig {
            active: Some("ghost".to_string()),
            ..Default::default()
        };
        assert!(!dangling.is_plaintext_omitted());
    }

    #[test]
    fn validate_rejects_dangling_active_with_no_certs() {
        let cfg = ConfSyncSecurityConfig {
            active: Some("ghost".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("dangling active must be rejected");
        assert!(
            format!("{err:?}").contains("active"),
            "error should mention 'active', got: {err:?}"
        );
    }

    // ── Group H: rotation configuration ──────────────────────────────────────

    #[test]
    fn rotation_defaults_are_spec_values() {
        let r = ConfSyncRotationConfig::default();
        assert!(matches!(r.policy, RotationPolicy::Lifetime));
        assert_eq!(r.lifetime_ratio, 0.5);
        assert_eq!(r.schedule, None);
        assert_eq!(r.schedule_timezone, "UTC");
        assert_eq!(r.rotate_if_remaining_below, CertDuration::from_hours(336));
        assert_eq!(r.emergency_rotate_below, CertDuration::from_hours(24));
        assert_eq!(r.leaf_ttl, CertDuration::from_hours(365 * 24));
        assert_eq!(r.ca_ttl, CertDuration::from_hours(3650 * 24));
        assert!(r.validate_numeric().is_ok());
    }

    #[test]
    fn rotation_parses_from_security_block() {
        let parsed: ConfSyncSecurityConfig = serde_json::from_str(
            r#"{"auto_ca": true, "rotation": {"policy": "scheduled", "schedule": "0 3 * * *", "leaf_ttl": "30d"}}"#,
        )
        .unwrap();
        let r = parsed.rotation.unwrap();
        assert!(matches!(r.policy, RotationPolicy::Scheduled));
        assert_eq!(r.leaf_ttl, CertDuration::parse("30d").unwrap());
    }

    #[test]
    fn rotation_numeric_validation_rejects_bad_combos() {
        let ok = ConfSyncRotationConfig::default();
        // churn hole: guard > 0.5 * leaf_ttl
        let mut bad = ok.clone();
        bad.policy = RotationPolicy::Scheduled;
        bad.schedule = Some("0 3 * * *".into());
        bad.leaf_ttl = CertDuration::parse("169h").unwrap();
        bad.rotate_if_remaining_below = CertDuration::parse("168h").unwrap();
        bad.emergency_rotate_below = CertDuration::parse("1h").unwrap();
        assert!(bad.validate_numeric().is_err());
        // lifetime: emergency net would always preempt the lifetime trigger
        let mut bad = ok.clone();
        bad.lifetime_ratio = 0.999;
        assert!(bad.validate_numeric().is_err());
        // ratio bounds
        for ratio in [0.0, 1.0, -0.1, 1.5] {
            let mut bad = ok.clone();
            bad.lifetime_ratio = ratio;
            assert!(bad.validate_numeric().is_err());
        }
        // scheduled requires a schedule
        let mut bad = ok.clone();
        bad.policy = RotationPolicy::Scheduled;
        bad.schedule = None;
        assert!(bad.validate_numeric().is_err());
        // ttl ordering
        let mut bad = ok.clone();
        bad.leaf_ttl = CertDuration::parse("11y").unwrap(); // > default ca_ttl (3650d)
        assert!(bad.validate_numeric().is_err());
    }
}
