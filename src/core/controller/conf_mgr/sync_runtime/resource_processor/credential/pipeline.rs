//! Top-level credential resolution pipeline.
//!
//! Orchestrates the credential resolution flow:
//! 1. Compute effective Secret namespace.
//! 2. Register SecretRefManager dependency (always — even if Secret absent or
//!    cross-namespace `ReferenceGrant` not yet present).
//! 3. Cross-namespace check via `ReferenceGrantStore`.
//! 4. Query Secret from `runtime.secret_store`.
//! 5. Validate `Secret.type` against the resolver's whitelist.
//! 6. Extract `Secret.data`.
//! 7. Delegate to [`SecretMaterialResolver::parse`] for keys/decode checks.
//!
//! Three public entry points:
//! - [`fetch_secret`] — steps 1–4 only; returns the raw `Secret`. Used by
//!   plugin resolvers that do their own data-shape extraction (EdgionPlugins).
//! - [`resolve_secret`] — full pipeline; returns the validated `Secret`.
//! - [`resolve_credential`] — full pipeline; returns the parsed `Material`.

use k8s_openapi::api::core::v1::Secret;

use crate::types::credentials::{ResolveError, SecretMaterialResolver};
use crate::types::resources::common::SecretReference;

use super::super::{format_secret_key, HandlerContext, ResourceRef};

// ---------------------------------------------------------------------------
// Public outcome types
// ---------------------------------------------------------------------------

/// Why resolution is deferred (not yet settled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingReason {
    /// The Secret object does not exist yet (or has not been cached).
    SecretNotFound { namespace: String, name: String },

    /// A cross-namespace reference requires a `ReferenceGrant` that is not
    /// (yet) present.
    ReferenceGrantMissing {
        from_ns: String,
        to_ns: String,
        secret_name: String,
    },
}

/// Why a credential is permanently invalid (caller should set `Invalid`
/// status rather than requeue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidCredentialError {
    /// One or more required keys are absent from the Secret data.
    MissingKeys(Vec<&'static str>),

    /// A key's value could not be decoded (e.g., non-UTF-8).
    DecodeError { key: &'static str, detail: String },

    /// The `Secret.type` is not in the resolver's accepted whitelist.
    InvalidSecretType {
        got: String,
        allowed: &'static [&'static str],
    },
}

/// Three-way result of the pipeline.
#[derive(Debug)]
pub enum ResolveOutcome<T> {
    /// Successfully resolved.
    Ok(T),

    /// Resolution is deferred — caller should set `Pending` status and wait
    /// for the referenced resource to appear / be granted.
    Pending(PendingReason),

    /// The Secret exists but its content is invalid — caller should set
    /// `Invalid` status; retrying without a Secret change won't help.
    Invalid(InvalidCredentialError),
}

// ---------------------------------------------------------------------------
// Internal steps (shared by the three public entry points)
// ---------------------------------------------------------------------------

/// Fetch the Secret object: compute namespace, register dependency, check
/// cross-namespace ReferenceGrant, look up in the store.
///
/// Returns `Ok(Secret)`, `Pending(SecretNotFound)`, or
/// `Pending(ReferenceGrantMissing)`. Never returns `Invalid`.
///
/// Callers that only need the raw Secret (e.g. EdgionPlugins plugin
/// resolvers) use this directly; the typed pipeline entry points
/// (`resolve_secret`, `resolve_credential`) layer type and key validation
/// on top via `resolve_inner`.
pub fn fetch_secret(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretReference,
) -> ResolveOutcome<Secret> {
    // ── Step 1: compute effective namespace ──────────────────────────────
    let owner_ns = owner.namespace.as_deref().unwrap_or("default");
    let secret_ns = secret_ref.effective_namespace(owner_ns).to_string();

    // ── Step 2: register SecretRefManager dependency ─────────────────────
    // Before Grant check and Secret lookup: owners must be tracked for Secret
    // lifecycle events even while Pending. ReferenceGrant changes requeue via
    // `ReferenceGrantStore`, not SecretRefManager.
    let secret_key = format_secret_key(Some(&secret_ns), &secret_ref.name);
    ctx.secret_ref_manager().add_ref(secret_key, owner.clone());

    // ── Step 3: cross-namespace ReferenceGrant check ─────────────────────
    if secret_ref.is_cross_namespace(owner_ns) {
        let allowed = ctx.runtime().ref_grant_store.check_reference_allowed(
            owner_ns,
            owner_group,
            owner_kind,
            &secret_ns,
            "",
            "Secret",
            Some(&secret_ref.name),
        );
        if !allowed {
            return ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns: owner_ns.to_string(),
                to_ns: secret_ns,
                secret_name: secret_ref.name.clone(),
            });
        }
    }

    // ── Step 4: look up the Secret ───────────────────────────────────────
    match ctx
        .runtime()
        .secret_store
        .get(Some(secret_ns.as_str()), &secret_ref.name)
    {
        Some(s) => ResolveOutcome::Ok(s),
        None => ResolveOutcome::Pending(PendingReason::SecretNotFound {
            namespace: secret_ns,
            name: secret_ref.name.clone(),
        }),
    }
}

/// Validate `Secret.type` against the resolver's whitelist. Missing type is
/// matched against the sentinel `""` entry.
fn check_secret_type<R: SecretMaterialResolver>(secret: &Secret, resolver: &R) -> Option<InvalidCredentialError> {
    let allowed = resolver.allowed_secret_types();
    let got = secret.type_.as_deref().unwrap_or("");
    if allowed.contains(&got) {
        None
    } else {
        Some(InvalidCredentialError::InvalidSecretType {
            got: got.to_string(),
            allowed,
        })
    }
}

fn map_parse_error(err: ResolveError) -> InvalidCredentialError {
    match err {
        ResolveError::MissingKeys(k) => InvalidCredentialError::MissingKeys(k),
        ResolveError::DecodeError { key, source } => InvalidCredentialError::DecodeError {
            key,
            detail: format!("{source}"),
        },
        ResolveError::InvalidSecretType { got, allowed } => InvalidCredentialError::InvalidSecretType { got, allowed },
    }
}

/// Shared body for `resolve_secret` and `resolve_credential`.
/// Runs the full pipeline (fetch + type check + parse) and returns the
/// validated `(Secret, Material)` pair.
fn resolve_inner<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretReference,
    resolver: &R,
) -> ResolveOutcome<(Secret, R::Material)> {
    let secret = match fetch_secret(ctx, owner, owner_group, owner_kind, secret_ref) {
        ResolveOutcome::Ok(s) => s,
        ResolveOutcome::Pending(r) => return ResolveOutcome::Pending(r),
        ResolveOutcome::Invalid(e) => return ResolveOutcome::Invalid(e),
    };

    if let Some(e) = check_secret_type(&secret, resolver) {
        return ResolveOutcome::Invalid(e);
    }

    let data = match secret.data.as_ref() {
        Some(d) => d,
        None => {
            return ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(resolver.required_keys().to_vec()));
        }
    };

    match resolver.parse(data) {
        Result::Ok(m) => ResolveOutcome::Ok((secret, m)),
        Err(e) => ResolveOutcome::Invalid(map_parse_error(e)),
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Resolve a credential Secret and return the validated [`Secret`] object.
///
/// Primary entry point for handlers that attach the raw Secret to a CRD
/// spec for gRPC sync to the Gateway. The resolver validates keys, types,
/// and encoding; its `Material` output is discarded here.
pub async fn resolve_secret<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretReference,
    resolver: &R,
) -> ResolveOutcome<Secret> {
    match resolve_inner(ctx, owner, owner_group, owner_kind, secret_ref, resolver) {
        ResolveOutcome::Ok((secret, _)) => ResolveOutcome::Ok(secret),
        ResolveOutcome::Pending(r) => ResolveOutcome::Pending(r),
        ResolveOutcome::Invalid(e) => ResolveOutcome::Invalid(e),
    }
}

/// Resolve a credential Secret and return the parsed `Material`.
///
/// Kept for unit tests and any call site that needs the typed material
/// rather than the raw Secret. New call sites should prefer
/// [`resolve_secret`] unless the Material is specifically required.
pub async fn resolve_credential<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretReference,
    resolver: &R,
) -> ResolveOutcome<R::Material> {
    match resolve_inner(ctx, owner, owner_group, owner_kind, secret_ref, resolver) {
        ResolveOutcome::Ok((_, material)) => ResolveOutcome::Ok(material),
        ResolveOutcome::Pending(r) => ResolveOutcome::Pending(r),
        ResolveOutcome::Invalid(e) => ResolveOutcome::Invalid(e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;

    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ResourceRef, SecretRefManager,
    };
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::credentials::BasicAuthResolver;
    use crate::types::ResourceKind;

    fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(
            secret_ref_manager,
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            runtime.clone(),
        );
        (ctx, runtime)
    }

    fn make_secret_with_type(
        namespace: &str,
        name: &str,
        type_: Option<&str>,
        data: &[(&str, &[u8])],
    ) -> (String, Secret) {
        let mut bmap = BTreeMap::new();
        for (k, v) in data {
            bmap.insert(k.to_string(), ByteString(v.to_vec()));
        }
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(bmap),
            type_: type_.map(str::to_string),
            ..Default::default()
        };
        (format!("{}/{}", namespace, name), secret)
    }

    fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
        make_secret_with_type(namespace, name, None, data)
    }

    fn owner_ref(ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(ResourceKind::LinkSys, Some(ns.to_string()), name.to_string())
    }

    fn secret_ref(name: &str, namespace: Option<&str>) -> SecretReference {
        SecretReference {
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn test_secret_ref_manager_registration() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let _ = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        let refs = ctx.secret_ref_manager().get_refs("default/my-secret");
        assert!(refs.contains(&owner), "owner must be registered, got {refs:?}");
    }

    #[tokio::test]
    async fn test_secret_not_found_returns_pending() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("missing-secret", None);

        let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing-secret");
            }
            other => panic!("expected Pending(SecretNotFound), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_missing_keys_invalid() {
        let (key, secret) = make_secret("default", "my-secret", &[("username", b"alice")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
                assert!(keys.contains(&"password"), "got {keys:?}");
            }
            other => panic!("expected Invalid(MissingKeys), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_happy_path_returns_ok() {
        let (key, secret) = make_secret(
            "default",
            "my-secret",
            &[("username", b"alice"), ("password", b"s3cr3t")],
        );
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Ok(m) => {
                assert_eq!(m.username.as_deref(), Some("alice"));
                assert_eq!(m.password, "s3cr3t");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_secret_returns_secret_object() {
        let (key, secret) = make_secret("default", "my-secret", &[("password", b"s3cr3t")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Ok(s) => {
                assert_eq!(s.metadata.name.as_deref(), Some("my-secret"));
                let data = s.data.expect("data");
                assert!(data.contains_key("password"));
            }
            other => panic!("expected Ok(Secret), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_basic_auth_accepts_k8s_basic_auth_type() {
        let (key, secret) = make_secret_with_type(
            "default",
            "my-secret",
            Some("kubernetes.io/basic-auth"),
            &[("password", b"s3cr3t")],
        );
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;
        assert!(
            matches!(outcome, ResolveOutcome::Ok(_)),
            "basic-auth type must be accepted, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_basic_auth_rejects_tls_type() {
        let (key, secret) = make_secret_with_type(
            "default",
            "my-secret",
            Some("kubernetes.io/tls"),
            &[("password", b"s3cr3t")],
        );
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", None);

        let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;
        match outcome {
            ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, .. }) => {
                assert_eq!(got, "kubernetes.io/tls");
            }
            other => panic!("expected InvalidSecretType, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_cross_ns_no_grant_returns_pending() {
        let (key, secret) = make_secret("other-ns", "my-secret", &[("password", b"s3cr3t")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("my-secret", Some("other-ns"));

        let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }) => {
                assert_eq!(from_ns, "default");
                assert_eq!(to_ns, "other-ns");
                assert_eq!(secret_name, "my-secret");
            }
            other => panic!("expected Pending(ReferenceGrantMissing), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_secret_ref_manager_registered_when_cross_ns_grant_missing() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let owner = owner_ref("default", "my-linksys");
        let sref = secret_ref("creds", Some("other-ns"));

        let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

        match outcome {
            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing { .. }) => {}
            other => panic!("expected Pending(ReferenceGrantMissing), got {other:?}"),
        }

        let refs = ctx.secret_ref_manager().get_refs("other-ns/creds");
        assert!(
            refs.contains(&owner),
            "owner must be registered for Secret wakeups, got {refs:?}"
        );
    }
}
