//! Generic authorization primitives shared by the Admin API.
//!
//! `Authorizer` decides `(role, verb, kind) -> Decision`. The Center role's
//! policy is held behind an `ArcSwap` so requests can share a policy snapshot without
//! restarting the federation client. Default-deny: an empty policy authorizes
//! nothing. Pure, no I/O.

mod access;
pub mod lint;

pub use access::{AccessSnapshot, AccessSnapshotError};

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::{ArcSwap, ArcSwapOption};

use crate::config::{FedAllowRule, FedRbacConfig};

/// The closed set of verbs the classifier can produce. Safe as a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Authenticated self-introspection. This is an inherent middleware action,
    /// not an RBAC-configurable verb; `CenterPolicy::permits` always rejects it.
    AccessSelf,
    Get,
    List,
    ListKeys,
    Watch,
    Create,
    Update,
    Delete,
    Failover,
    AcmeTrigger,
    ConfSyncRotate,
    ServerInfo,
    Diagnostics,
    WipeAll,
    Debug,
}

impl Verb {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verb::AccessSelf => "access-self",
            Verb::Get => "get",
            Verb::List => "list",
            Verb::ListKeys => "list-keys",
            Verb::Watch => "watch",
            Verb::Create => "create",
            Verb::Update => "update",
            Verb::Delete => "delete",
            Verb::Failover => "failover",
            Verb::AcmeTrigger => "acme-trigger",
            Verb::ConfSyncRotate => "conf-sync-rotate",
            Verb::ServerInfo => "server-info",
            Verb::Diagnostics => "diagnostics",
            Verb::WipeAll => "wipe-all",
            Verb::Debug => "debug",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Verb> {
        Some(match s {
            // AccessSelf is intentionally absent: operators cannot grant or
            // revoke authenticated self-introspection through RBAC config.
            "get" => Verb::Get,
            "list" => Verb::List,
            "list-keys" => Verb::ListKeys,
            "watch" => Verb::Watch,
            "create" => Verb::Create,
            "update" => Verb::Update,
            "delete" => Verb::Delete,
            "failover" => Verb::Failover,
            "acme-trigger" => Verb::AcmeTrigger,
            "conf-sync-rotate" => Verb::ConfSyncRotate,
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
        if verb == Verb::AccessSelf {
            return false;
        }
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
    /// config-derived map) so it survives every `apply` — including repeated configuration assembly,
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
    /// Subsequent `apply` calls re-merge it automatically.
    pub fn set_admin_token(&self, entry: Option<(String, CliTokenEntry)>) {
        self.admin_token.store(entry.map(Arc::new));
    }

    /// Hot-swap the Center policy (per-request snapshots pick this up).
    pub fn swap_policy(&self, policy: CenterPolicy) {
        self.center_policy.store(Arc::new(policy));
    }

    /// Atomically replace both the Center policy and the cli-token map.
    ///
    /// This is the single writer path used for startup configuration assembly.
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
    /// stable across the lifetime of a single request across authorization checks.
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

impl Role {
    /// Build the bounded effective-access document from this request's stable
    /// policy Arc. The endpoint handler must use the Role already injected into
    /// request extensions rather than reloading Authorizer state.
    pub fn access_snapshot(&self) -> Result<AccessSnapshot, AccessSnapshotError> {
        access::snapshot(self)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/authz/mod_test.rs"]
mod tests;
