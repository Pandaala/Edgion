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
//! Four entry points:
//! - [`fetch_secret`] — steps 1–4 only; returns the raw `Secret`. Used by
//!   plugin resolvers that do their own data-shape extraction (EdgionPlugins).
//! - [`resolve_secret`] — full pipeline; returns the validated `Secret`.
//! - [`resolve_credential`] — full pipeline; returns the parsed `Material`.
//! - [`resolve_bound_secret_readonly`] — validated owning-field lookup without reverse
//!   dependency registration, for status derivation after parse.

use k8s_openapi::api::core::v1::{ConfigMap, Secret};

use edgion_resources::credentials::AiApiKeyResolver;
use edgion_resources::credentials::{ResolveError, SecretMaterialResolver};
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::refs::backend_fields::BackendSecretField;
use edgion_resources::resources::refs::{BoundFieldReference, ReferenceError, ReferenceToken};

use super::super::ref_grant::read_after_registering_cross_ns_dependency;
use super::super::{format_configmap_ref_key, format_secret_key, HandlerContext, ResourceRef};

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

    /// A required value decoded successfully but violates its fixed syntax.
    InvalidValue {
        key: &'static str,
        class: edgion_resources::credentials::InvalidValueClass,
    },

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

/// Two-way result of [`fetch_secret`]. Fetching never produces an `Invalid`
/// outcome (type/key validation happens in later pipeline stages), so this
/// type is strictly narrower than [`ResolveOutcome`].
#[derive(Debug)]
pub enum FetchOutcome<T> {
    /// Successfully fetched.
    Ok(T),

    /// Fetch is deferred — caller should set `Pending` status and wait for
    /// the referenced Secret to appear or for the cross-namespace
    /// `ReferenceGrant` to be granted.
    Pending(PendingReason),
}

// Production expands directly to the operation; test recording lives in the
// mirrored module and preserves the same evaluation order and return value.
#[cfg(not(test))]
macro_rules! bound_read {
    ($boundary:literal, $operation:expr) => {
        $operation
    };
}
#[cfg(test)]
use tests::bound_read;

/// A lookup from an actual owning-field loan. Invalid identities never create
/// dependency edges or reach an object store.
#[derive(Debug)]
pub enum BoundFetchOutcome<T> {
    Ok(T),
    Pending(PendingReason),
    Invalid(ReferenceError),
}
impl<T> BoundFetchOutcome<T> {
    fn and_then<U>(self, map: impl FnOnce(T) -> BoundFetchOutcome<U>) -> BoundFetchOutcome<U> {
        match self {
            Self::Ok(value) => map(value),
            Self::Pending(reason) => BoundFetchOutcome::Pending(reason),
            Self::Invalid(error) => BoundFetchOutcome::Invalid(error),
        }
    }
}

/// Secret and authorization captured by one lookup. Async credential inputs
/// retain this revision instead of independently querying permission again.
pub struct AuthorizedSecret {
    pub secret: Secret,
    pub authorization_revision: String,
}

impl std::fmt::Debug for AuthorizedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AuthorizedSecret").finish_non_exhaustive()
    }
}

pub fn fetch_bound_secret(
    ctx: &HandlerContext,
    dependency_owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<AuthorizedSecret> {
    fetch_bound_secret_inner(ctx, dependency_owner, token, true)
}

pub(crate) fn fetch_bound_secret_readonly(
    ctx: &HandlerContext,
    dependency_owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<AuthorizedSecret> {
    fetch_bound_secret_inner(ctx, dependency_owner, token, false)
}

/// Status uses the same bound lookup as parse without adding recovery edges.
pub(crate) fn bound_secret_status_error(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> Option<super::super::ResolvedRefsError> {
    bound_secret_status_result(ctx, owner, token).err()
}

pub(crate) fn bound_secret_status_result(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    use super::super::ResolvedRefsError;
    match fetch_bound_secret_readonly(ctx, owner, token) {
        BoundFetchOutcome::Ok(authorized) => Ok(authorized),
        BoundFetchOutcome::Invalid(error) => Err(ResolvedRefsError::InvalidReference { error }),
        BoundFetchOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            Err(ResolvedRefsError::SecretNotFound { namespace, name })
        }
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: owner.kind_str(),
            target_namespace: to_ns,
            target_name: secret_name,
        }),
    }
}

/// Resolve typed material while retaining the first lookup's authorizer revision.
pub fn resolve_bound_secret<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
    resolver: &R,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    resolve_bound_outcome(fetch_bound_secret(ctx, owner, token), owner, resolver)
}

/// Resolve an actual Backend credential field through the shared identity,
/// dependency, authorization and material-validation pipeline.
pub(crate) fn resolve_backend_secret(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    field: &BackendSecretField<'_>,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    let outcome = match field.validated() {
        Ok(reference) => fetch_bound_reference(ctx, owner, reference, true),
        Err(error) => BoundFetchOutcome::Invalid(error),
    };
    resolve_bound_outcome(outcome, owner, &AiApiKeyResolver)
}

pub(crate) fn resolve_bound_secret_readonly<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
    resolver: &R,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    resolve_bound_outcome(fetch_bound_secret_readonly(ctx, owner, token), owner, resolver)
}

/// Read only the generated output derived from a real LinkSys-backed Injector.
pub(crate) fn resolve_managed_credential_output(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
    register: bool,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    let outcome = match token.managed_credential_output() {
        Ok(reference) => fetch_bound_reference(ctx, owner, reference, register),
        Err(error) => BoundFetchOutcome::Invalid(error),
    };
    resolve_bound_outcome(outcome, owner, &super::BearerResolver)
}

fn resolve_bound_outcome<R: SecretMaterialResolver>(
    outcome: BoundFetchOutcome<AuthorizedSecret>,
    owner: &ResourceRef,
    resolver: &R,
) -> Result<AuthorizedSecret, super::super::ResolvedRefsError> {
    use super::super::ResolvedRefsError;
    let authorized = match outcome {
        BoundFetchOutcome::Ok(authorized) => authorized,
        BoundFetchOutcome::Invalid(error) => return Err(ResolvedRefsError::InvalidReference { error }),
        BoundFetchOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            return Err(ResolvedRefsError::SecretNotFound { namespace, name })
        }
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => {
            return Err(ResolvedRefsError::RefNotPermitted {
                from_namespace: from_ns,
                from_kind: owner.kind_str(),
                target_namespace: to_ns,
                target_name: secret_name,
            })
        }
    };
    validate_secret(&authorized.secret, resolver).map_err(|error| match error {
        InvalidCredentialError::MissingKeys(keys) => ResolvedRefsError::MissingKeys { keys },
        InvalidCredentialError::DecodeError { key, detail } => ResolvedRefsError::DecodeError { key, detail },
        InvalidCredentialError::InvalidValue { key, class } => ResolvedRefsError::DecodeError {
            key,
            detail: class.to_string(),
        },
        InvalidCredentialError::InvalidSecretType { got, allowed } => {
            ResolvedRefsError::InvalidSecretType { got, allowed }
        }
    })?;
    Ok(authorized)
}

fn fetch_bound_secret_inner(
    ctx: &HandlerContext,
    dependency_owner: &ResourceRef,
    token: ReferenceToken<'_>,
    register: bool,
) -> BoundFetchOutcome<AuthorizedSecret> {
    let reference = match token.validated() {
        Ok(reference) => reference,
        Err(error) => return BoundFetchOutcome::Invalid(error),
    };
    fetch_bound_reference(ctx, dependency_owner, reference, register)
}

#[derive(Clone, Copy)]
enum CoreMaterialKind {
    Secret,
    ConfigMap,
}
impl CoreMaterialKind {
    fn kind(self) -> &'static str {
        match self {
            Self::Secret => "Secret",
            Self::ConfigMap => "ConfigMap",
        }
    }
}
struct AuthorizedObject<'a> {
    namespace: &'a str,
    name: &'a str,
    revision: String,
}

fn authorize_bound_object<'a>(
    ctx: &HandlerContext,
    dependency_owner: &ResourceRef,
    reference: BoundFieldReference<'a>,
    register: bool,
    expected: CoreMaterialKind,
) -> BoundFetchOutcome<AuthorizedObject<'a>> {
    let owner = reference.owner();
    if dependency_owner.kind_str() != owner.group_kind().kind()
        || dependency_owner.name != owner.name()
        || dependency_owner.namespace.as_deref().unwrap_or("default") != owner.scope().namespace().unwrap_or("")
    {
        return BoundFetchOutcome::Invalid(ReferenceError::StaleBinding);
    }
    let target = reference.target();
    if target.group_kind().group() != "" || target.group_kind().kind() != expected.kind() {
        return BoundFetchOutcome::Invalid(ReferenceError::UnsupportedTarget);
    }
    let Some(namespace) = target.scope().namespace() else {
        return BoundFetchOutcome::Invalid(ReferenceError::MissingNamespace);
    };
    if register {
        let key = match expected {
            CoreMaterialKind::Secret => format!("{namespace}/{}", target.name()),
            CoreMaterialKind::ConfigMap => format!(
                "{}{namespace}/{}",
                super::super::CONFIG_MAP_REF_KEY_PREFIX,
                target.name()
            ),
        };
        ctx.secret_ref_manager().add_ref(key, dependency_owner.clone());
        if !reference.is_same_namespace() {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(namespace.to_string(), dependency_owner.clone());
        }
    }
    let Some(revision) = bound_read!(
        "authorization",
        ctx.runtime()
            .ref_grant_store
            .query_authorization_revision(reference.grant_query())
    ) else {
        return BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns: reference.source().scope().namespace().unwrap_or("").to_string(),
            to_ns: namespace.to_string(),
            secret_name: target.name().to_string(),
        });
    };
    BoundFetchOutcome::Ok(AuthorizedObject {
        namespace,
        name: target.name(),
        revision,
    })
}

fn fetch_bound_reference(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    reference: BoundFieldReference<'_>,
    register: bool,
) -> BoundFetchOutcome<AuthorizedSecret> {
    authorize_bound_object(ctx, owner, reference, register, CoreMaterialKind::Secret).and_then(|object| {
        match bound_read!(
            "secret",
            ctx.runtime().secret_store.get(Some(object.namespace), object.name)
        ) {
            Some(secret) => BoundFetchOutcome::Ok(AuthorizedSecret {
                secret: super::ca_refs::ordinary_secret_material(secret),
                authorization_revision: object.revision,
            }),
            None => BoundFetchOutcome::Pending(PendingReason::SecretNotFound {
                namespace: object.namespace.to_string(),
                name: object.name.to_string(),
            }),
        }
    })
}

pub(crate) fn fetch_bound_configmap(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<ConfigMap> {
    fetch_bound_configmap_inner(ctx, owner, token, true)
}

pub(crate) fn fetch_bound_configmap_readonly(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<ConfigMap> {
    fetch_bound_configmap_inner(ctx, owner, token, false)
}

pub(crate) fn bound_configmap_status_result(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> Result<ConfigMap, super::super::ResolvedRefsError> {
    use super::super::ResolvedRefsError;
    match fetch_bound_configmap_readonly(ctx, owner, token) {
        BoundFetchOutcome::Ok(configmap) => Ok(configmap),
        BoundFetchOutcome::Invalid(error) => Err(ResolvedRefsError::InvalidReference { error }),
        BoundFetchOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            Err(ResolvedRefsError::ConfigMapNotFound { namespace, name })
        }
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: owner.kind_str(),
            target_namespace: to_ns,
            target_name: secret_name,
        }),
    }
}

fn fetch_bound_configmap_inner(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
    register: bool,
) -> BoundFetchOutcome<ConfigMap> {
    let reference = match token.validated() {
        Ok(reference) => reference,
        Err(error) => return BoundFetchOutcome::Invalid(error),
    };
    authorize_bound_object(ctx, owner, reference, register, CoreMaterialKind::ConfigMap).and_then(|object| {
        match bound_read!(
            "configmap",
            ctx.runtime().configmap_store.get(Some(object.namespace), object.name)
        ) {
            Some(configmap) => BoundFetchOutcome::Ok(configmap),
            None => BoundFetchOutcome::Pending(PendingReason::SecretNotFound {
                namespace: object.namespace.to_string(),
                name: object.name.to_string(),
            }),
        }
    })
}

// ---------------------------------------------------------------------------
// Internal steps (shared by the four entry points)
// ---------------------------------------------------------------------------

/// Fetch the Secret object: compute namespace, register dependency, check
/// cross-namespace ReferenceGrant, look up in the store.
///
/// Returns [`FetchOutcome::Ok`] or [`FetchOutcome::Pending`] (with
/// `SecretNotFound` or `ReferenceGrantMissing`). The narrower
/// [`FetchOutcome`] return type makes the absence of an `Invalid` arm
/// explicit; callers should not write a fallback `Invalid` branch.
///
/// Callers that only need the raw Secret (e.g. EdgionPlugins plugin
/// resolvers) use this directly; the typed pipeline entry points
/// (`resolve_secret`, `resolve_credential`, `resolve_credential_readonly`)
/// layer type and key validation on top via `resolve_inner`.
pub fn fetch_secret(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretObjectReference,
) -> FetchOutcome<Secret> {
    fetch_secret_inner(ctx, owner, owner_group, owner_kind, secret_ref, true)
}

fn fetch_secret_inner(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretObjectReference,
    register_dependency: bool,
) -> FetchOutcome<Secret> {
    // ── Step 1: compute effective namespace ──────────────────────────────
    let owner_ns = owner.namespace.as_deref().unwrap_or("default");
    let secret_ns = secret_ref.effective_namespace(owner_ns).to_string();

    // ── Step 2: register SecretRefManager dependency ─────────────────────
    // Before Grant check and Secret lookup: owners must be tracked for Secret
    // lifecycle events even while Pending. Cross-namespace refs are also
    // indexed so a ReferenceGrant change requeues the owner.
    if register_dependency {
        let secret_key = format_secret_key(Some(&secret_ns), &secret_ref.name);
        ctx.secret_ref_manager().add_ref(secret_key, owner.clone());
        if secret_ref.is_cross_namespace(owner_ns) {
            ctx.runtime().cross_ns_ref_mgr.add_ref(secret_ns.clone(), owner.clone());
        }
    }

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
            return FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
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
        Some(s) => FetchOutcome::Ok(super::ca_refs::ordinary_secret_material(s)),
        None => FetchOutcome::Pending(PendingReason::SecretNotFound {
            namespace: secret_ns,
            name: secret_ref.name.clone(),
        }),
    }
}

/// Fetch the ConfigMap object: register dependency, optionally check cross-namespace
/// ReferenceGrant, then look up in the store.
///
/// Mirrors [`fetch_secret`] for ConfigMaps. The `namespace` parameter is the
/// explicit namespace of the target ConfigMap (`None` means "same as owner"). The
/// `owner_ns` parameter is the owner resource's own namespace (`None` for
/// cluster-scoped resources). When `namespace` is `Some` and differs from
/// `owner_ns` the function checks for a cross-namespace `ReferenceGrant` authorised
/// by `owner_group`/`owner_kind` before accessing the store.
///
/// `owner_group` and `owner_kind` identify the requesting resource type on the
/// "from" side of the `ReferenceGrant` (`from.group`, `from.kind`).
pub fn fetch_configmap(
    ctx: &HandlerContext,
    resource_ref: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    name: &str,
    namespace: Option<&str>,
    owner_ns: Option<&str>,
) -> FetchOutcome<ConfigMap> {
    // ── Step 1: compute effective namespace ──────────────────────────────
    let Some(effective_ns) = namespace.or(owner_ns) else {
        // Cluster-scoped owner with no explicit namespace: cannot resolve.
        return FetchOutcome::Pending(PendingReason::SecretNotFound {
            namespace: String::new(),
            name: name.to_string(),
        });
    };

    // ── Step 2: register SecretRefManager dependency ─────────────────────
    let key_ns = effective_ns.to_string();
    let cm_key = format_configmap_ref_key(Some(&key_ns), name);
    ctx.secret_ref_manager().add_ref(cm_key, resource_ref.clone());

    // ── Step 3: cross-namespace ReferenceGrant check ─────────────────────
    let cross_ns = match (namespace, owner_ns) {
        (Some(ns), Some(own)) => ns != own,
        (Some(_), None) => true, // cluster-scoped owner referencing an explicit namespace
        (None, _) => false,      // same-namespace reference
    };
    if cross_ns {
        let from_ns = owner_ns.unwrap_or("");
        let allowed = read_after_registering_cross_ns_dependency(ctx.runtime(), effective_ns, resource_ref, || {
            ctx.runtime().ref_grant_store.check_reference_allowed(
                from_ns,
                owner_group,
                owner_kind,
                effective_ns,
                "",
                "ConfigMap",
                Some(name),
            )
        });
        if !allowed {
            return FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns: from_ns.to_string(),
                to_ns: effective_ns.to_string(),
                secret_name: name.to_string(),
            });
        }
    }

    // ── Step 4: look up the ConfigMap ────────────────────────────────────
    match ctx.runtime().configmap_store.get(Some(effective_ns), name) {
        Some(cm) => FetchOutcome::Ok(cm),
        None => FetchOutcome::Pending(PendingReason::SecretNotFound {
            namespace: effective_ns.to_string(),
            name: name.to_string(),
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
        ResolveError::InvalidValue { key, class } => InvalidCredentialError::InvalidValue { key, class },
        ResolveError::InvalidSecretType { got, allowed } => InvalidCredentialError::InvalidSecretType { got, allowed },
    }
}

fn validate_secret<R: SecretMaterialResolver>(
    secret: &Secret,
    resolver: &R,
) -> Result<R::Material, InvalidCredentialError> {
    if let Some(error) = check_secret_type(secret, resolver) {
        return Err(error);
    }
    let data = secret
        .data
        .as_ref()
        .ok_or_else(|| InvalidCredentialError::MissingKeys(resolver.required_keys().to_vec()))?;
    resolver.parse(data).map_err(map_parse_error)
}

/// Shared body for `resolve_secret` and `resolve_credential`.
/// Runs the full pipeline (fetch + type check + parse) and returns the
/// validated `(Secret, Material)` pair.
fn resolve_inner<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretObjectReference,
    resolver: &R,
) -> ResolveOutcome<(Secret, R::Material)> {
    resolve_inner_with_registration(ctx, owner, owner_group, owner_kind, secret_ref, resolver, true)
}

fn resolve_inner_with_registration<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    owner_group: &str,
    owner_kind: &str,
    secret_ref: &SecretObjectReference,
    resolver: &R,
    register_dependency: bool,
) -> ResolveOutcome<(Secret, R::Material)> {
    let secret = match fetch_secret_inner(ctx, owner, owner_group, owner_kind, secret_ref, register_dependency) {
        FetchOutcome::Ok(s) => s,
        FetchOutcome::Pending(r) => return ResolveOutcome::Pending(r),
    };

    match validate_secret(&secret, resolver) {
        Result::Ok(m) => ResolveOutcome::Ok((secret, m)),
        Err(e) => ResolveOutcome::Invalid(e),
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
    secret_ref: &SecretObjectReference,
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
    secret_ref: &SecretObjectReference,
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/credential/pipeline_test.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::observe_bound_reads;
