//! Generic authorization primitives shared by the Admin API.
//!
//! `Authorizer` decides `(role, verb, kind) -> Decision`. The Center role's
//! policy is held behind an `ArcSwap` so it can be hot-swapped on reload without
//! restarting the federation client. Default-deny: an empty policy authorizes
//! nothing. Pure, no I/O.

pub mod lint;

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::{ArcSwap, ArcSwapOption};

use crate::config::{FedAllowRule, FedRbacConfig};

/// The closed set of verbs the classifier can produce. Safe as a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    List,
    ListKeys,
    Watch,
    Create,
    Update,
    Delete,
    Failover,
    AcmeTrigger,
    Reload,
    ServerInfo,
    Diagnostics,
    WipeAll,
    Debug,
}

impl Verb {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verb::Get => "get",
            Verb::List => "list",
            Verb::ListKeys => "list-keys",
            Verb::Watch => "watch",
            Verb::Create => "create",
            Verb::Update => "update",
            Verb::Delete => "delete",
            Verb::Failover => "failover",
            Verb::AcmeTrigger => "acme-trigger",
            Verb::Reload => "reload",
            Verb::ServerInfo => "server-info",
            Verb::Diagnostics => "diagnostics",
            Verb::WipeAll => "wipe-all",
            Verb::Debug => "debug",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Verb> {
        Some(match s {
            "get" => Verb::Get,
            "list" => Verb::List,
            "list-keys" => Verb::ListKeys,
            "watch" => Verb::Watch,
            "create" => Verb::Create,
            "update" => Verb::Update,
            "delete" => Verb::Delete,
            "failover" => Verb::Failover,
            "acme-trigger" => Verb::AcmeTrigger,
            "reload" => Verb::Reload,
            "server-info" => Verb::ServerInfo,
            "diagnostics" => Verb::Diagnostics,
            "wipe-all" => Verb::WipeAll,
            "debug" => Verb::Debug,
            _ => return None,
        })
    }
}

/// The identity assigned to a request.
///
/// `Clone` is required because `authz_layer` clones the role from request
/// extensions; axum extensions require `Clone + Send + Sync + 'static`.
#[derive(Debug, Clone)]
pub enum Role {
    /// A role whose access is evaluated against an attached `CenterPolicy`.
    /// Used for the federation Center identity, the local admin token (which
    /// carries an allow-all wildcard policy), and scoped cli tokens. There is no
    /// special allow-all variant: an "admin" identity is simply a `Restricted`
    /// role holding a wildcard policy.
    Restricted(Arc<CenterPolicy>),
}

#[derive(Debug, Clone)]
struct CompiledRule {
    verbs: Vec<String>,
    kinds: Vec<String>,
}

/// Default-deny policy for the `Restricted` role.
#[derive(Debug, Clone, Default)]
pub struct CenterPolicy {
    rules: Vec<CompiledRule>,
}

impl CenterPolicy {
    pub fn deny_all() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn from_config(cfg: &FedRbacConfig) -> Self {
        let rules = cfg
            .allow
            .iter()
            .map(|r| CompiledRule {
                verbs: r.verbs.iter().map(|s| s.to_ascii_lowercase()).collect(),
                kinds: r.kinds.clone(),
            })
            .collect();
        Self { rules }
    }

    fn permits(&self, verb: Verb, kind: &str) -> bool {
        let v = verb.as_str();
        self.rules
            .iter()
            .any(|r| r.verbs.iter().any(|x| x == "*" || x == v) && r.kinds.iter().any(|x| x == "*" || x == kind))
    }
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    pub fn allowed(self) -> bool {
        self == Decision::Allow
    }
}

/// An entry in the cli token table.
///
/// The map key is `sha256_hex(raw_token)`; this struct holds the human-readable
/// name (for audit logging) and the per-token RBAC policy.
#[derive(Debug, Clone)]
pub struct CliTokenEntry {
    pub name: String,
    pub policy: Arc<CenterPolicy>,
}

/// Compute the lowercase hex-encoded SHA-256 digest of a token string.
///
/// Used to build the cli-token lookup table (stored by hash, never raw value)
/// and by the pre-authn middleware to look up an incoming Bearer token.
pub fn sha256_hex(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// Reserved name for the operator-supplied local admin token entry.
///
/// The local admin token (Istio-style) authenticates `edgion-cli` and local
/// automation. It carries an allow-all wildcard policy and is registered into the
/// same cli-token map as the scoped Center tokens, distinguished only by this
/// name and its policy.
pub const ADMIN_TOKEN_NAME: &str = "admin";

/// The allow-all (`* × *`) policy carried by the local admin token.
///
/// Built through `CenterPolicy::from_config` because the policy's rules are
/// private; `permits()` treats `"*"` as match-all for both verbs and kinds.
pub fn admin_wildcard_policy() -> CenterPolicy {
    CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["*".to_string()],
            kinds: vec!["*".to_string()],
        }],
    })
}

/// Build the `(hash, entry)` for the local admin token, or `None` when no token
/// is configured.
///
/// Rejects blank / whitespace-only tokens (a security guard: otherwise
/// `sha256_hex("")` would register an allow-all token that authenticates an empty
/// `Authorization: Bearer ` header). The returned entry carries the wildcard
/// admin policy and the reserved `ADMIN_TOKEN_NAME`.
pub fn build_admin_token_entry(raw: Option<&str>) -> Option<(String, CliTokenEntry)> {
    let tok = raw?.trim();
    if tok.is_empty() {
        return None;
    }
    let entry = CliTokenEntry {
        name: ADMIN_TOKEN_NAME.to_string(),
        policy: Arc::new(admin_wildcard_policy()),
    };
    Some((sha256_hex(tok), entry))
}

/// Shared authorizer holding a hot-swappable Center policy and a cli-token map.
///
/// Construction (`new`) always accepts a single `CenterPolicy` argument so
/// existing call sites continue to compile. The cli-token map defaults to empty
/// and is installed (or refreshed) via `apply`.
pub struct Authorizer {
    center_policy: ArcSwap<CenterPolicy>,
    /// Key = `sha256_hex(raw_token)`.  Value = name + per-token policy.
    cli_tokens: ArcSwap<HashMap<String, CliTokenEntry>>,
    /// The local admin token entry, if configured. Held here (rather than in the
    /// config-derived map) so it survives every `apply` — including hot-reloads,
    /// which rebuild the cli-token map purely from `config.cli_tokens` and would
    /// otherwise silently drop it.
    admin_token: ArcSwapOption<(String, CliTokenEntry)>,
}

impl Authorizer {
    /// Create a new `Authorizer` with the given Center policy and an empty
    /// cli-token map. Call `apply` to install the initial cli-token set.
    pub fn new(policy: CenterPolicy) -> Self {
        Self {
            center_policy: ArcSwap::from_pointee(policy),
            cli_tokens: ArcSwap::from_pointee(HashMap::new()),
            admin_token: ArcSwapOption::const_empty(),
        }
    }

    /// Install (or clear) the local admin token entry. Must be called BEFORE the
    /// first `apply` at startup so the admin token is merged into the live map.
    /// Subsequent `apply` calls (reloads) re-merge it automatically.
    pub fn set_admin_token(&self, entry: Option<(String, CliTokenEntry)>) {
        self.admin_token.store(entry.map(Arc::new));
    }

    /// Hot-swap the Center policy (per-request snapshots pick this up).
    pub fn swap_policy(&self, policy: CenterPolicy) {
        self.center_policy.store(Arc::new(policy));
    }

    /// Atomically replace both the Center policy and the cli-token map.
    ///
    /// This is the single writer path used at startup and on every hot-reload.
    /// The configured admin token (if any) is merged into `cli` so it always
    /// remains live; an existing map entry with the same hash (operator reusing
    /// the same raw token as a cli_token) is left untouched.
    pub fn apply(&self, center: CenterPolicy, mut cli: HashMap<String, CliTokenEntry>) {
        if let Some(admin) = self.admin_token.load_full() {
            let (hash, entry) = admin.as_ref();
            cli.entry(hash.clone()).or_insert_with(|| entry.clone());
        }
        self.center_policy.store(Arc::new(center));
        self.cli_tokens.store(Arc::new(cli));
    }

    /// Snapshot the current Center policy as a `Role::Restricted`.
    ///
    /// The snapshot is load-once per call (ArcSwap `load_full`), so it is
    /// stable across the lifetime of a single request even if a reload races.
    pub fn center_role(&self) -> Role {
        Role::Restricted(self.center_policy.load_full())
    }

    /// Look up an incoming raw Bearer token in the cli-token table.
    ///
    /// Returns `Some((Role::Restricted(policy), name))` on a hit, `None` on a
    /// miss. The lookup hashes the token before consulting the map.
    pub fn cli_lookup(&self, token: &str) -> Option<(Role, String)> {
        let h = sha256_hex(token);
        self.cli_tokens
            .load()
            .get(&h)
            .map(|e| (Role::Restricted(e.policy.clone()), e.name.clone()))
    }

    /// Returns `true` when at least one cli token is registered.
    ///
    /// Used by `/api/v1/auth/status` to advertise the `"cli"` active provider.
    pub fn cli_tokens_present(&self) -> bool {
        !self.cli_tokens.load().is_empty()
    }

    pub fn authorize(&self, role: &Role, verb: Verb, kind: &str) -> Decision {
        match role {
            Role::Restricted(p) => {
                if p.permits(verb, kind) {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FedAllowRule, FedRbacConfig};

    fn policy(rules: Vec<(&[&str], &[&str])>) -> CenterPolicy {
        CenterPolicy::from_config(&FedRbacConfig {
            allow: rules
                .into_iter()
                .map(|(v, k)| FedAllowRule {
                    verbs: v.iter().map(|s| s.to_string()).collect(),
                    kinds: k.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        })
    }

    #[test]
    fn empty_policy_denies_all() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(!az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
    }

    #[test]
    fn center_policy_matches_exact_and_wildcard() {
        let az = Authorizer::new(policy(vec![
            (&["get", "list"], &["PluginMetaData"]),
            (&["reload"], &["*"]),
        ]));
        assert!(az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
        assert!(!az
            .authorize(&az.center_role(), Verb::Delete, "PluginMetaData")
            .allowed());
        assert!(az.authorize(&az.center_role(), Verb::Reload, "AnythingHere").allowed());
        assert!(!az.authorize(&az.center_role(), Verb::Get, "Secret").allowed());
    }

    #[test]
    fn policy_hot_swaps() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(!az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
        az.swap_policy(policy(vec![(&["get"], &["PluginMetaData"])]));
        assert!(az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
    }

    #[test]
    fn center_policy_verb_is_case_insensitive() {
        let az = Authorizer::new(policy(vec![(&["GET", "List"], &["PluginMetaData"])]));
        assert!(az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
        assert!(az.authorize(&az.center_role(), Verb::List, "PluginMetaData").allowed());
    }

    #[test]
    fn verb_roundtrips_string() {
        assert_eq!(Verb::Get.as_str(), "get");
        assert_eq!(Verb::from_str("failover"), Some(Verb::Failover));
        assert_eq!(Verb::from_str("nonsense"), None);
    }

    // --- New Task 1 tests ---

    #[test]
    fn restricted_role_uses_its_own_policy() {
        let p = CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["get".into()],
                kinds: vec!["HTTPRoute".into()],
            }],
        });
        let role = Role::Restricted(std::sync::Arc::new(p));
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(az.authorize(&role, Verb::Get, "HTTPRoute").allowed());
        assert!(!az.authorize(&role, Verb::Delete, "HTTPRoute").allowed());
    }

    #[test]
    fn cli_lookup_resolves_by_sha256() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        let p = CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["get".into()],
                kinds: vec!["PluginMetaData".into()],
            }],
        });
        let mut m = std::collections::HashMap::new();
        let hash = crate::authz::sha256_hex("tok-abc");
        m.insert(
            hash,
            CliTokenEntry {
                name: "bot".into(),
                policy: std::sync::Arc::new(p),
            },
        );
        az.apply(CenterPolicy::deny_all(), m);
        let (role, name) = az.cli_lookup("tok-abc").expect("hit");
        assert_eq!(name, "bot");
        assert!(az.authorize(&role, Verb::Get, "PluginMetaData").allowed());
        assert!(az.cli_lookup("wrong").is_none());
        assert!(az.cli_tokens_present());
    }

    #[test]
    fn center_role_snapshots_policy() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(!az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
        az.swap_policy(CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["get".into()],
                kinds: vec!["PluginMetaData".into()],
            }],
        }));
        assert!(az.authorize(&az.center_role(), Verb::Get, "PluginMetaData").allowed());
    }

    // --- Local admin token (Istio-style) ---

    #[test]
    fn admin_wildcard_policy_allows_everything() {
        let p = admin_wildcard_policy();
        let role = Role::Restricted(Arc::new(p));
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(az.authorize(&role, Verb::Delete, "Secret").allowed());
        assert!(az.authorize(&role, Verb::WipeAll, "AnythingHere").allowed());
    }

    #[test]
    fn build_admin_token_entry_rejects_blank() {
        assert!(build_admin_token_entry(None).is_none());
        assert!(build_admin_token_entry(Some("")).is_none());
        assert!(build_admin_token_entry(Some("   ")).is_none());
        let (hash, entry) = build_admin_token_entry(Some("  tok-xyz  ")).expect("non-blank token");
        // Whitespace is trimmed before hashing.
        assert_eq!(hash, sha256_hex("tok-xyz"));
        assert_eq!(entry.name, ADMIN_TOKEN_NAME);
    }

    #[test]
    fn admin_token_survives_apply_reload() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        az.set_admin_token(build_admin_token_entry(Some("admin-secret")));

        // Startup apply with an empty config map: admin token must be live.
        az.apply(CenterPolicy::deny_all(), HashMap::new());
        let (role, name) = az
            .cli_lookup("admin-secret")
            .expect("admin token present after startup");
        assert_eq!(name, ADMIN_TOKEN_NAME);
        assert!(az.authorize(&role, Verb::Delete, "Secret").allowed());

        // Reload apply rebuilds the map from config (no admin token) — it must
        // still be re-merged automatically.
        let mut reload_map = HashMap::new();
        reload_map.insert(
            sha256_hex("center-bot"),
            CliTokenEntry {
                name: "bot".into(),
                policy: Arc::new(CenterPolicy::deny_all()),
            },
        );
        az.apply(CenterPolicy::deny_all(), reload_map);
        assert!(
            az.cli_lookup("admin-secret").is_some(),
            "admin token must survive reload"
        );
        assert!(az.cli_lookup("center-bot").is_some(), "center token also present");
    }

    #[test]
    fn no_admin_token_means_no_extra_entry() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        az.set_admin_token(build_admin_token_entry(Some("   "))); // blank → None
        az.apply(CenterPolicy::deny_all(), HashMap::new());
        assert!(!az.cli_tokens_present());
    }
}
