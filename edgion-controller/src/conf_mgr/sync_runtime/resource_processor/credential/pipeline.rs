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
//! Owning-field entry points:
//! - [`fetch_bound_secret`] and `fetch_bound_configmap` acquire validated field targets.
//! - [`resolve_bound_secret`] validates material and retains the authorizer revision.
//! - [`resolve_bound_secret_readonly`] derives status without dependency writes.
//!
//! Field validation and exact dependency-owner matching precede registration,
//! authorization and store access. Specialized ACME and managed-credential paths
//! retain their own existing bootstrap and publication contracts.

use k8s_openapi::api::core::v1::{ConfigMap, Secret};

use edgion_resources::credentials::AiApiKeyResolver;
use edgion_resources::credentials::{ResolveError, SecretMaterialResolver};
use edgion_resources::resources::refs::backend_fields::BackendSecretField;
use edgion_resources::resources::refs::{BoundFieldReference, ReferenceError, ReferenceToken};

use super::super::{HandlerContext, ResourceRef};

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

/// Preserve ACME's bootstrap and credential diagnostics while binding acquisition.
pub(crate) fn resolve_bound_acme_account(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    field: &edgion_resources::resources::refs::acme_fields::AcmeAccountField<'_>,
) -> Result<ResolveOutcome<Secret>, ReferenceError> {
    let authorized = match fetch_bound_secret(ctx, owner, field.token()) {
        BoundFetchOutcome::Invalid(error) => return Err(error),
        BoundFetchOutcome::Pending(reason) => return Ok(ResolveOutcome::Pending(reason)),
        BoundFetchOutcome::Ok(value) => value,
    };
    Ok(
        match validate_secret(&authorized.secret, &edgion_resources::credentials::AcmeAccountResolver) {
            Ok(_) => ResolveOutcome::Ok(authorized.secret),
            Err(error) => ResolveOutcome::Invalid(error),
        },
    )
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
    let outcome = outcome.and_then(
        |authorized| match token.validate_managed_credential_secret(&authorized.secret) {
            Ok(()) => BoundFetchOutcome::Ok(authorized),
            Err(error) => BoundFetchOutcome::Invalid(error),
        },
    );
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
        if reference.is_gateway_frontend_ca() {
            use super::super::frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind};
            let kind = match expected {
                CoreMaterialKind::Secret => FrontendCaSourceKind::Secret,
                CoreMaterialKind::ConfigMap => FrontendCaSourceKind::ConfigMap,
            };
            ctx.runtime().frontend_ca_ref_manager.add_ref(
                FrontendCaSourceKey::new(kind, namespace, target.name()).index_key(),
                dependency_owner.clone(),
            );
        } else {
            let key = match expected {
                CoreMaterialKind::Secret => format!("{namespace}/{}", target.name()),
                CoreMaterialKind::ConfigMap => format!(
                    "{}{namespace}/{}",
                    super::super::CONFIG_MAP_REF_KEY_PREFIX,
                    target.name()
                ),
            };
            ctx.secret_ref_manager().add_ref(key, dependency_owner.clone());
        }
        if !reference.is_same_namespace() {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(namespace.to_string(), dependency_owner.clone());
        }
    }
    // Preserve the existing controller policy only for these finite Gateway and
    // EdgionTls fields. Their synchronous loans cannot mint frozen authority.
    if (reference.is_gateway_tls() || reference.is_edgion_tls_material())
        && !crate::config::is_reference_grant_validation_enabled()
    {
        return BoundFetchOutcome::Ok(AuthorizedObject {
            namespace,
            name: target.name(),
            revision: if reference.is_gateway_tls() {
                "gateway-validation-disabled"
            } else {
                "edgion-tls-validation-disabled"
            }
            .to_string(),
        });
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

/// Authorize an actual frontend CA slot without reading its payload (fallback mode).
pub(crate) fn authorize_bound_gateway_frontend_ca(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<()> {
    let reference = match token.validated() {
        Ok(reference) => reference,
        Err(error) => return BoundFetchOutcome::Invalid(error),
    };
    if !reference.is_gateway_frontend_ca() {
        return BoundFetchOutcome::Invalid(ReferenceError::UnsupportedTarget);
    }
    let kind = match reference.target().group_kind().kind() {
        "Secret" => CoreMaterialKind::Secret,
        "ConfigMap" => CoreMaterialKind::ConfigMap,
        _ => return BoundFetchOutcome::Invalid(ReferenceError::UnsupportedTarget),
    };
    authorize_bound_object(ctx, owner, reference, true, kind).and_then(|_| BoundFetchOutcome::Ok(()))
}

/// Readonly authorization for an actual EdgionTls field, without payload acquisition.
pub(crate) fn authorize_bound_edgion_tls(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: ReferenceToken<'_>,
) -> BoundFetchOutcome<()> {
    let reference = match token.validated() {
        Ok(value) => value,
        Err(error) => return BoundFetchOutcome::Invalid(error),
    };
    if !reference.is_edgion_tls_material() {
        return BoundFetchOutcome::Invalid(ReferenceError::UnsupportedTarget);
    }
    authorize_bound_object(ctx, owner, reference, false, CoreMaterialKind::Secret)
        .and_then(|_| BoundFetchOutcome::Ok(()))
}

// Default's two cached reads are not an atomic store snapshot. Dependencies are
// registered before acquisition so later source events reconcile either candidate.
fn read_bound_material<T: kube::Resource>(
    reference: BoundFieldReference<'_>,
    object: &AuthorizedObject<'_>,
    mut read: impl FnMut(Option<&str>, &str) -> Option<T>,
) -> BoundFetchOutcome<T> {
    let qualified = read(Some(object.namespace), object.name);
    let selected = if reference.is_gateway_tls()
        || reference.is_backend_tls_material()
        || reference.is_health_tls_material()
        || reference.is_edgion_tls_material()
        || reference.is_acme_account()
    {
        let bare = if object.namespace == "default" {
            read(None, object.name)
        } else {
            None
        };
        let (material, raw_namespace) = match (qualified, bare) {
            (Some(_), Some(_)) => return BoundFetchOutcome::Invalid(ReferenceError::StaleBinding),
            (Some(material), None) => (Some(material), Some(object.namespace)),
            (None, Some(material)) => (Some(material), None),
            (None, None) => (None, None),
        };
        if let Some(material) = material.as_ref() {
            let metadata = material.meta();
            if metadata.name.as_deref() != Some(object.name) || metadata.namespace.as_deref() != raw_namespace {
                return BoundFetchOutcome::Invalid(ReferenceError::StaleBinding);
            }
        }
        material
    } else {
        // Other consumers retain exact-store and frozen-authority semantics.
        qualified
    };
    match selected {
        Some(material) => BoundFetchOutcome::Ok(material),
        None => BoundFetchOutcome::Pending(PendingReason::SecretNotFound {
            namespace: object.namespace.to_string(),
            name: object.name.to_string(),
        }),
    }
}

fn fetch_bound_reference(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    reference: BoundFieldReference<'_>,
    register: bool,
) -> BoundFetchOutcome<AuthorizedSecret> {
    authorize_bound_object(ctx, owner, reference, register, CoreMaterialKind::Secret).and_then(|object| {
        read_bound_material(reference, &object, |namespace, name| {
            bound_read!("secret", ctx.runtime().secret_store.get(namespace, name))
        })
        .and_then(|secret| {
            BoundFetchOutcome::Ok(AuthorizedSecret {
                secret: super::ca_refs::ordinary_secret_material(secret),
                authorization_revision: object.revision,
            })
        })
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
        read_bound_material(reference, &object, |namespace, name| {
            bound_read!("configmap", ctx.runtime().configmap_store.get(namespace, name))
        })
    })
}

// ---------------------------------------------------------------------------
// Shared material validation
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/credential/pipeline_test.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::observe_bound_reads;
