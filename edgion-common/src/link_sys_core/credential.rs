//! Materialise credentials on the Gateway from the embedded `Secret` carried
//! inside `LinkSys.spec.*Auth.secret`.
//!
//! # Four-state credential model
//!
//! Auth credentials go through four states on the Gateway side:
//!
//! - [`CredentialState::NotRequired`] — the CRD does not configure auth at all.
//! - [`CredentialState::Pending`]     — the CRD configures `auth.secretRef`, but
//!   the Controller has not yet injected the `Secret` object into `auth.secret`.
//! - [`CredentialState::Invalid`]     — the `Secret` is injected but the
//!   reference identity or resolver rejected its material (missing keys / decode error / wrong type).
//! - [`CredentialState::Resolved`]    — material is usable.
//!
//! Dispatch gates on [`is_buildable`]; providers receive a
//! `&CredentialState<_>` so that Pending / Invalid produce a fail-fast `bail!`
//! instead of a silent empty-credential fallback.

use anyhow::{bail, Result};
use edgion_resources::credentials::materials::{
    BasicAuthMaterial, BearerMaterial, EsApiKeyMaterial, HmacMaterial, WebhookApiKeyMaterial,
};
use edgion_resources::credentials::resolvers::{BasicAuthResolver, BearerResolver, EsApiKeyResolver};
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchAuth;
use edgion_resources::resources::link_sys::otlp::OtlpBearerResolver;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use edgion_resources::resources::refs::{link_sys_fields::PreparedLinkSysReferences, ReferenceError};
use k8s_openapi::api::core::v1::Secret;

// ============================================================================
// CredentialState & InvalidReason
// ============================================================================

/// Explicit reason for why a [`CredentialState::Invalid`] was produced.
///
/// Reference identity failures have a fixed reason; parsing failures retain the
/// resolver message. The string payload is suitable for `tracing` but must not
/// be surfaced in user-visible error responses (may contain key names from
/// `Secret.data`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    /// Actual reference and carried material do not match. Contains no submitted values.
    ReferenceIdentity,
    /// The resolver failed to turn `Secret.data` into a valid `Material`.
    ParseFailed(String),
}

impl std::fmt::Display for InvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReferenceIdentity => f.write_str("credential reference identity mismatch"),
            Self::ParseFailed(e) => write!(f, "parse failed: {e}"),
        }
    }
}

/// Four-state view of a credential on the Gateway side.
///
/// Generic over the material type `M` so the same enum shape serves both
/// Redis/Etcd basic auth (`CredentialState<BasicAuthMaterial>`) and
/// Elasticsearch/Webhook multi-variant auth
/// (`CredentialState<ResolvedCredential>`).
///
/// # Security
///
/// The derived [`Debug`] impl **redacts** the `Resolved` variant's material so
/// that `tracing::info!(state = ?state, ...)` cannot accidentally leak
/// passwords / tokens via structured logs. Pattern-match directly when the
/// material is needed.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialState<M> {
    /// CRD does not configure auth → provider must not emit any credential.
    NotRequired,
    /// CRD configures auth via `secretRef`, but the Controller has not yet
    /// attached the `Secret` object (transient on the happy path).
    Pending,
    /// `Secret` is attached but reference identity or material parsing failed — a hard
    /// error state that will not resolve without an operator fix.
    Invalid(InvalidReason),
    /// Material parsed successfully.
    Resolved(M),
}

impl<M> CredentialState<M> {
    /// True when dispatch can safely build a client with this state —
    /// [`NotRequired`](Self::NotRequired) or [`Resolved`](Self::Resolved).
    #[inline]
    pub fn is_buildable(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Resolved(_))
    }

    /// Stable short identifier for logs / metrics — never includes material.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotRequired => "NotRequired",
            Self::Pending => "Pending",
            Self::Invalid(_) => "Invalid",
            Self::Resolved(_) => "Resolved",
        }
    }

    /// Map the inner material (only in the `Resolved` variant).
    fn map<N, F: FnOnce(M) -> N>(self, f: F) -> CredentialState<N> {
        match self {
            Self::NotRequired => CredentialState::NotRequired,
            Self::Pending => CredentialState::Pending,
            Self::Invalid(r) => CredentialState::Invalid(r),
            Self::Resolved(m) => CredentialState::Resolved(f(m)),
        }
    }
}

impl<M> std::fmt::Debug for CredentialState<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRequired => f.write_str("NotRequired"),
            Self::Pending => f.write_str("Pending"),
            Self::Invalid(r) => write!(f, "Invalid({r:?})"),
            Self::Resolved(_) => f.write_str("Resolved(<redacted>)"),
        }
    }
}

impl<M> std::fmt::Display for CredentialState<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRequired => f.write_str("NotRequired"),
            Self::Pending => f.write_str("Pending"),
            Self::Invalid(r) => write!(f, "Invalid({r})"),
            Self::Resolved(_) => f.write_str("Resolved"),
        }
    }
}

// ============================================================================
// ResolvedCredential — the material type stored in CredentialState for ES / Webhook
// ============================================================================

/// Materialised credential for Elasticsearch / Webhook dispatch.
///
/// Variant selection is driven by the CRD's auth kind; [`resolved_state`]
/// guarantees the variant always matches the CRD's declaration, so providers
/// do not need to guard against "type mismatch" at call time.
#[derive(Debug, Clone)]
pub enum ResolvedCredential {
    Basic(BasicAuthMaterial),
    EsApiKey(EsApiKeyMaterial),
    Bearer(BearerMaterial),
    Hmac(HmacMaterial),
    WebhookApiKey(WebhookApiKeyMaterial),
}

// ============================================================================
// Parsing helpers
// ============================================================================

/// Parse a `Secret` using the supplied resolver. `Ok(material)` on success;
/// `Err` on `data == None` or any `ResolveError` (preserving the `ResolveError`
/// as the `anyhow::Error` source chain).
fn parse_secret<R: SecretMaterialResolver>(secret: &Secret, resolver: &R) -> Result<R::Material, anyhow::Error> {
    let Some(data) = secret.data.as_ref() else {
        return Err(anyhow::anyhow!("Secret.data is empty"));
    };
    resolver.parse(data).map_err(anyhow::Error::new)
}

/// Check the actual active auth slot on a cold configuration path.
/// The returned material borrows the supplied owner; no material is copied.
pub fn checked_auth_secret(ls: &LinkSys) -> Result<Option<&Secret>, ReferenceError> {
    PreparedLinkSysReferences::from_link_sys(ls).resolved_auth_secret(ls)
}

fn state_from_auth_secret<R: SecretMaterialResolver>(ls: &LinkSys, resolver: &R) -> CredentialState<R::Material> {
    let secret = match checked_auth_secret(ls) {
        Ok(Some(secret)) => secret,
        Ok(None) => return CredentialState::Pending,
        Err(_) => return CredentialState::Invalid(InvalidReason::ReferenceIdentity),
    };
    match parse_secret(secret, resolver) {
        Ok(m) => CredentialState::Resolved(m),
        Err(e) => {
            tracing::warn!(error = ?e, "failed to parse credential material on Gateway");
            CredentialState::Invalid(InvalidReason::ParseFailed(format!("{e:#}")))
        }
    }
}

// ============================================================================
// Per-provider state accessors
// ============================================================================

/// Credential state for Redis / Etcd basic auth.
///
/// Non-Redis / non-Etcd `LinkSys` always return [`CredentialState::NotRequired`].
pub fn basic_state(ls: &LinkSys) -> CredentialState<BasicAuthMaterial> {
    let has_auth = match &ls.spec.config {
        SystemConfig::Redis(r) => r.auth.is_some(),
        SystemConfig::Etcd(e) => e.auth.is_some(),
        _ => return CredentialState::NotRequired,
    };
    if !has_auth {
        return CredentialState::NotRequired;
    }
    state_from_auth_secret(ls, &BasicAuthResolver)
}

/// Credential state for Elasticsearch / Webhook auth.
///
/// The resolver is selected from the CRD's auth variant, so a
/// `Resolved(ResolvedCredential::Bearer(_))` result against
/// `ElasticsearchAuth::Basic` is impossible by construction — callers on the
/// Provider side may rely on this invariant.
pub fn resolved_state(ls: &LinkSys) -> CredentialState<ResolvedCredential> {
    match &ls.spec.config {
        SystemConfig::Elasticsearch(es) => match es.auth.as_ref() {
            None => CredentialState::NotRequired,
            Some(ElasticsearchAuth::Basic { .. }) => {
                state_from_auth_secret(ls, &BasicAuthResolver).map(ResolvedCredential::Basic)
            }
            Some(ElasticsearchAuth::ApiKey { .. }) => {
                state_from_auth_secret(ls, &EsApiKeyResolver).map(ResolvedCredential::EsApiKey)
            }
            Some(ElasticsearchAuth::Bearer { .. }) => {
                state_from_auth_secret(ls, &BearerResolver).map(ResolvedCredential::Bearer)
            }
        },
        SystemConfig::Otlp(otlp) => match &otlp.auth {
            None => CredentialState::NotRequired,
            Some(_) => state_from_auth_secret(ls, &OtlpBearerResolver).map(ResolvedCredential::Bearer),
        },
        SystemConfig::Webhook(_) => CredentialState::NotRequired,
        SystemConfig::Kafka(kafka) => match kafka.sasl.as_ref() {
            None => CredentialState::NotRequired,
            Some(_) => state_from_auth_secret(ls, &BasicAuthResolver).map(ResolvedCredential::Basic),
        },
        _ => CredentialState::NotRequired,
    }
}

// ============================================================================
// Fail-fast credential helpers
// ============================================================================

/// Helper: turn a `CredentialState<BasicAuthMaterial>` into a fail-fast result.
///
/// - `Resolved(m)` → `Ok(Some(m))`
/// - `NotRequired` → `Ok(None)` —— caller decides whether to flag "CRD declared
///   auth but state is NotRequired" as an internal-bug (Redis/Etcd do).
/// - `Pending` / `Invalid` → `bail!` with a uniform message keyed on `provider`.
pub fn require_basic_material<'a>(
    state: &'a CredentialState<BasicAuthMaterial>,
    provider: &'static str,
) -> Result<Option<&'a BasicAuthMaterial>> {
    match state {
        CredentialState::Resolved(m) => Ok(Some(m)),
        CredentialState::NotRequired => Ok(None),
        CredentialState::Pending => bail!(
            "{provider} auth is configured via secretRef but credential material \
             has not been resolved yet; refusing to build client without credentials"
        ),
        CredentialState::Invalid(reason) => bail!("{provider} credential material is invalid: {reason}"),
    }
}

/// Helper: same shape as [`require_basic_material`] but for ES/Webhook-style
/// `CredentialState<ResolvedCredential>` where caller always requires a
/// resolved material (no `NotRequired` early return).
///
/// Caller is responsible for separately checking `CredentialState::NotRequired`
/// upstream when relevant.
pub fn require_resolved<'a>(
    state: &'a CredentialState<ResolvedCredential>,
    provider: &'static str,
) -> Result<&'a ResolvedCredential> {
    match state {
        CredentialState::Resolved(m) => Ok(m),
        CredentialState::NotRequired => {
            bail!("{provider} auth configured in CRD but credential state is NotRequired (internal bug)")
        }
        CredentialState::Pending => bail!(
            "{provider} auth is configured via secretRef but credential material \
             has not been resolved yet; refusing to build client without credentials"
        ),
        CredentialState::Invalid(reason) => bail!("{provider} credential material is invalid: {reason}"),
    }
}

// ============================================================================
// TLS readiness (structural) & combined is_buildable
// ============================================================================

fn tls_ready_inner(tls: Option<&TlsConfig>) -> bool {
    let Some(tls) = tls else {
        return true;
    };
    if !tls.enabled {
        return true;
    }
    // Not ready if CA certificate refs are declared but the Controller has not yet
    // injected the resolved secrets.
    let has_ca_refs = tls
        .outbound
        .validation
        .as_ref()
        .is_some_and(|v| v.ca_certificate_refs.as_ref().is_some_and(|r| !r.is_empty()));
    if has_ca_refs && tls.resolved_ca_certificates.is_none() {
        return false;
    }
    // Not ready if a client-identity secret ref is declared but not yet resolved.
    if tls.outbound.client_certificate_ref.is_some() && tls.resolved_client_certificate.is_none() {
        return false;
    }
    true
}

/// TLS readiness check (structural only).
///
/// Disabled TLS blocks are inert. Returns `false` when an enabled block declares
/// CA or client-certificate references that the Controller has not resolved. TLS parse failures are
/// caught inside the provider via the outbound TLS resolver.
pub fn tls_ready(ls: &LinkSys) -> bool {
    // Exhaustive on purpose: a `_ => None` fallback would make any webhook-shaped
    // provider (Webhook / HttpDns) that declares CA/client-cert refs report
    // `tls_ready == true` before the Controller injected the secrets — a
    // fail-open readiness bug (DR-4). List every variant so a future one forces
    // an explicit decision here.
    let tls = match &ls.spec.config {
        SystemConfig::Unsupported => None,
        SystemConfig::Redis(r) => r.tls.as_ref(),
        SystemConfig::Etcd(e) => e.tls.as_ref(),
        SystemConfig::Otlp(o) => o.tls.as_ref(),
        SystemConfig::Elasticsearch(es) => es.tls.as_ref(),
        SystemConfig::Kafka(k) => k.tls.as_ref(),
        SystemConfig::Webhook(w) => w.conn.tls.as_ref(),
        SystemConfig::HttpDns(h) => h.connection.as_ref().and_then(|c| c.tls.as_ref()),
        SystemConfig::CredentialSource(source) => match &source.provider {
            edgion_resources::resources::link_sys::credential_source::CredentialSourceProvider::OAuth2ClientCredentials(
                oauth2,
            ) => oauth2.tls.as_ref(),
        },
    };
    tls_ready_inner(tls)
}

// Note: there is intentionally no top-level `is_buildable(ls)` helper that
// combines auth + TLS. Dispatch sites compose the two terms explicitly so
// they can log each axis separately (see `runtime::store` for the pattern
// `auth_state.is_buildable() && credential::tls_ready(ls)`). Providers only
// see `&CredentialState<_>`, they don't need the combined gate.

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/credential_test.rs"]
mod tests;
