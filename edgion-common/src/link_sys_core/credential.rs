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
//!   resolver rejected its `data` (missing keys / decode error / wrong type).
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
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use k8s_openapi::api::core::v1::Secret;

// ============================================================================
// CredentialState & InvalidReason
// ============================================================================

/// Explicit reason for why a [`CredentialState::Invalid`] was produced.
///
/// Currently only carries the resolver error message; kept as an enum so that
/// future reasons (e.g. `AuthTypeMismatch`) can be added without churning the
/// public signature. The string payload is suitable for `tracing` but must not
/// be surfaced in user-visible error responses (may contain key names from
/// `Secret.data`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    /// The resolver failed to turn `Secret.data` into a valid `Material`.
    ParseFailed(String),
}

impl std::fmt::Display for InvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
    /// `Secret` is attached but the resolver rejected its `data` — a hard
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

fn state_from_optional_secret<R: SecretMaterialResolver>(
    secret: Option<&Secret>,
    resolver: &R,
) -> CredentialState<R::Material> {
    let Some(secret) = secret else {
        return CredentialState::Pending;
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
    let (has_auth, secret) = match &ls.spec.config {
        SystemConfig::Redis(r) => (r.auth.is_some(), r.auth.as_ref().and_then(|a| a.slot.secret.as_ref())),
        SystemConfig::Etcd(e) => (e.auth.is_some(), e.auth.as_ref().and_then(|a| a.slot.secret.as_ref())),
        _ => return CredentialState::NotRequired,
    };
    if !has_auth {
        return CredentialState::NotRequired;
    }
    state_from_optional_secret(secret, &BasicAuthResolver)
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
            Some(ElasticsearchAuth::Basic { slot }) => {
                state_from_optional_secret(slot.secret.as_ref(), &BasicAuthResolver).map(ResolvedCredential::Basic)
            }
            Some(ElasticsearchAuth::ApiKey { slot }) => {
                state_from_optional_secret(slot.secret.as_ref(), &EsApiKeyResolver).map(ResolvedCredential::EsApiKey)
            }
            Some(ElasticsearchAuth::Bearer { slot }) => {
                state_from_optional_secret(slot.secret.as_ref(), &BearerResolver).map(ResolvedCredential::Bearer)
            }
        },
        SystemConfig::Webhook(_w) => {
            // Webhook auth is now a deprecated shim field (Option<serde_json::Value>)
            // that is accepted for backward compatibility but not used at runtime.
            // Credential processing will be re-implemented in Wave B with the new request field.
            CredentialState::NotRequired
        }
        SystemConfig::Kafka(kafka) => match kafka.sasl.as_ref() {
            None => CredentialState::NotRequired,
            Some(sasl) => state_from_optional_secret(sasl.password.secret.as_ref(), &BasicAuthResolver)
                .map(ResolvedCredential::Basic),
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
/// Returns `false` when the CRD declares a `clientCertificateRef` but the
/// Controller has not yet injected the `Secret`. TLS parse failures are
/// caught inside the provider via the outbound TLS resolver.
pub fn tls_ready(ls: &LinkSys) -> bool {
    // Exhaustive on purpose: a `_ => None` fallback would make any webhook-shaped
    // provider (Webhook / HttpDns) that declares CA/client-cert refs report
    // `tls_ready == true` before the Controller injected the secrets — a
    // fail-open readiness bug (DR-4). List every variant so a future one forces
    // an explicit decision here.
    let tls = match &ls.spec.config {
        SystemConfig::Redis(r) => r.tls.as_ref(),
        SystemConfig::Etcd(e) => e.tls.as_ref(),
        SystemConfig::Elasticsearch(es) => es.tls.as_ref(),
        SystemConfig::Kafka(k) => k.tls.as_ref(),
        SystemConfig::Webhook(w) => w.conn.tls.as_ref(),
        SystemConfig::HttpDns(h) => h.connection.as_ref().and_then(|c| c.tls.as_ref()),
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
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, TlsConfig as CommonTlsConfig};
    use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
    use edgion_resources::resources::link_sys::LinkSysSpec;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

    fn mk_secret(data: &[(&str, &[u8])]) -> Secret {
        let mut bmap = BTreeMap::new();
        for (k, v) in data {
            bmap.insert(k.to_string(), ByteString(v.to_vec()));
        }
        Secret {
            data: Some(bmap),
            ..Default::default()
        }
    }

    fn mk_redis_linksys(auth: Option<RedisAuth>) -> LinkSys {
        LinkSys {
            metadata: ObjectMeta {
                name: Some("r".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: vec!["redis://localhost:6379".into()],
                    auth,
                    db: None,
                    timeout: None,
                    pool: None,
                    retry: None,
                    topology: None,
                    tls: None,
                    observability: None,
                }),
            },
            status: None,
        }
    }

    fn mk_redis_auth(secret: Option<Secret>) -> RedisAuth {
        use edgion_resources::resources::common::SecretSlot;
        RedisAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                },
                secret,
            },
        }
    }

    // ------------------------------------------------------------
    // CredentialState helpers
    // ------------------------------------------------------------

    #[test]
    fn is_buildable_accepts_not_required_and_resolved() {
        let not_required: CredentialState<BasicAuthMaterial> = CredentialState::NotRequired;
        let resolved = CredentialState::Resolved(BasicAuthMaterial {
            username: None,
            password: "p".into(),
        });
        let pending: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
        let invalid: CredentialState<BasicAuthMaterial> =
            CredentialState::Invalid(InvalidReason::ParseFailed("boom".into()));

        assert!(not_required.is_buildable());
        assert!(resolved.is_buildable());
        assert!(!pending.is_buildable());
        assert!(!invalid.is_buildable());
    }

    #[test]
    fn debug_impl_redacts_resolved_material() {
        let s = CredentialState::Resolved(BasicAuthMaterial {
            username: Some("alice".into()),
            password: "hunter2".into(),
        });
        let rendered = format!("{s:?}");
        assert!(rendered.contains("Resolved"), "got {rendered}");
        assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
        assert!(!rendered.contains("alice"), "username leaked: {rendered}");
    }

    #[test]
    fn kind_returns_variant_label() {
        let pending: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
        assert_eq!(pending.kind(), "Pending");
        let resolved = CredentialState::Resolved(BasicAuthMaterial {
            username: None,
            password: "p".into(),
        });
        assert_eq!(resolved.kind(), "Resolved");
    }

    // ------------------------------------------------------------
    // basic_state — 4 variants
    // ------------------------------------------------------------

    #[test]
    fn basic_state_not_required_when_no_auth() {
        let ls = mk_redis_linksys(None);
        assert!(matches!(basic_state(&ls), CredentialState::NotRequired));
    }

    #[test]
    fn basic_state_pending_when_secret_missing() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(None)));
        assert!(matches!(basic_state(&ls), CredentialState::Pending));
    }

    #[test]
    fn basic_state_resolved_when_secret_present() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[
            ("username", b"u"),
            ("password", b"p"),
        ])))));
        let state = basic_state(&ls);
        let m = match state {
            CredentialState::Resolved(m) => m,
            other => panic!("expected Resolved, got {other:?}"),
        };
        assert_eq!(m.username.as_deref(), Some("u"));
        assert_eq!(m.password, "p");
    }

    #[test]
    fn basic_state_invalid_when_secret_malformed() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("wrong_key", b"x")])))));
        match basic_state(&ls) {
            CredentialState::Invalid(InvalidReason::ParseFailed(msg)) => {
                assert!(msg.contains("missing") || msg.contains("Missing") || msg.contains("password"));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// When a LinkSys with embedded secret is JSON-round-tripped (simulating
    /// conf_sync), Gateway can still materialise.
    #[test]
    fn basic_state_survives_conf_sync_roundtrip() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("password", b"hunter2")])))));
        let json = serde_json::to_string(&ls).unwrap();
        let ls2: LinkSys = serde_json::from_str(&json).unwrap();
        match basic_state(&ls2) {
            CredentialState::Resolved(m) => assert_eq!(m.password, "hunter2"),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    // ------------------------------------------------------------
    // resolved_state — ES variants
    // ------------------------------------------------------------

    fn mk_es_linksys(auth: Option<ElasticsearchAuth>) -> LinkSys {
        use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
        LinkSys {
            metadata: ObjectMeta {
                name: Some("es".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                    auth,
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[test]
    fn resolved_state_es_basic_resolved() {
        use edgion_resources::resources::common::SecretSlot;
        let ls = mk_es_linksys(Some(ElasticsearchAuth::Basic {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                },
                secret: Some(mk_secret(&[("username", b"u"), ("password", b"p")])),
            },
        }));
        let got = resolved_state(&ls);
        assert!(matches!(got, CredentialState::Resolved(ResolvedCredential::Basic(_))));
    }

    #[test]
    fn resolved_state_es_api_key_pending() {
        use edgion_resources::resources::common::SecretSlot;
        let ls = mk_es_linksys(Some(ElasticsearchAuth::ApiKey {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                },
                secret: None,
            },
        }));
        assert!(matches!(resolved_state(&ls), CredentialState::Pending));
    }

    #[test]
    fn resolved_state_es_bearer_invalid_on_missing_key() {
        use edgion_resources::resources::common::SecretSlot;
        let ls = mk_es_linksys(Some(ElasticsearchAuth::Bearer {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                },
                secret: Some(mk_secret(&[("nope", b"x")])),
            },
        }));
        assert!(matches!(
            resolved_state(&ls),
            CredentialState::Invalid(InvalidReason::ParseFailed(_))
        ));
    }

    #[test]
    fn resolved_state_no_auth_is_not_required() {
        let ls = mk_es_linksys(None);
        assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    }

    // ------------------------------------------------------------
    // resolved_state + dispatch gate — Webhook variants
    //
    // Webhook dispatch must gate on `resolved_state(ls).is_buildable()`
    // (same as Elasticsearch) so a webhook whose `auth.secretRef` Secret is
    // Pending / Invalid is not registered and cannot silently send an
    // unauthenticated request.
    // ------------------------------------------------------------

    fn mk_webhook_linksys(auth: Option<serde_json::Value>) -> LinkSys {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
        };
        LinkSys {
            metadata: ObjectMeta {
                name: Some("wh".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    conn: ExternalHttpConnection {
                        target: WebhookTarget {
                            service: WebhookServiceRef {
                                kind: None,
                                name: Some("webhook-svc".into()),
                                namespace: Some("default".into()),
                                port: Some(8080),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    auth,
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[test]
    fn resolved_state_webhook_no_auth_is_not_required() {
        let ls = mk_webhook_linksys(None);
        assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    }

    // ------------------------------------------------------------
    // DR-4: tls_ready must NOT fail open for the webhook-shaped
    // providers (Webhook / HttpDns) whose `connection.tls` declares
    // an unresolved client-certificate ref. Before the explicit arms
    // these fell through `_ => None` and reported ready == true.
    // ------------------------------------------------------------

    fn tls_with_unresolved_client_cert() -> edgion_resources::resources::common::TlsConfig {
        serde_json::from_str(r#"{"enabled":true,"clientCertificateRef":{"name":"id"}}"#).unwrap()
    }

    fn mk_httpdns_linksys(
        connection: Option<edgion_resources::resources::link_sys::httpdns::HttpDnsConnection>,
    ) -> LinkSys {
        use edgion_resources::resources::link_sys::httpdns::{
            HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
        };
        LinkSys {
            metadata: ObjectMeta {
                name: Some("dns".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::HttpDns(HttpDnsResolverConfig {
                    preset: Some(HttpDnsPreset::Aliyun),
                    url_template: None,
                    response: HttpDnsResponseFormat::default(),
                    fallback: HttpDnsFallback::System,
                    connection,
                }),
            },
            status: None,
        }
    }

    #[test]
    fn tls_ready_httpdns_false_when_client_cert_unresolved() {
        use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
        let ls = mk_httpdns_linksys(Some(HttpDnsConnection {
            tls: Some(tls_with_unresolved_client_cert()),
            timeout_ms: None,
        }));
        assert!(!tls_ready(&ls), "httpdns connection.tls readiness must not fail open");
    }

    #[test]
    fn tls_ready_httpdns_true_when_no_tls() {
        let ls = mk_httpdns_linksys(None);
        assert!(tls_ready(&ls), "no connection.tls means nothing to wait for");
    }

    #[test]
    fn tls_ready_webhook_false_when_client_cert_unresolved() {
        let mut ls = mk_webhook_linksys(None);
        if let SystemConfig::Webhook(w) = &mut ls.spec.config {
            w.conn.tls = Some(tls_with_unresolved_client_cert());
        }
        assert!(!tls_ready(&ls), "webhook conn.tls readiness must not fail open");
    }

    #[test]
    fn resolved_state_webhook_bearer_not_required_shim_field_unused() {
        // Webhook auth is now a deprecated shim field that is not used at runtime.
        // Credential resolution always returns NotRequired regardless of auth content.
        let ls = mk_webhook_linksys(None);
        assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    }

    #[test]
    fn resolved_state_webhook_bearer_shim_value_ignored() {
        // Webhook auth is now a deprecated shim field that is not used at runtime.
        // Even with a shim value, credential resolution returns NotRequired.
        let ls = mk_webhook_linksys(Some(serde_json::json!({
            "type": "bearer",
            "secretRef": {"name": "token"}
        })));
        assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    }

    /// The exact gate expression used by both Webhook dispatch arms in
    /// `runtime::store`. Locks "client is registered iff the credential is
    /// buildable".
    fn webhook_dispatch_gate(ls: &LinkSys) -> bool {
        resolved_state(ls).is_buildable()
    }

    #[test]
    fn webhook_dispatch_gate_open_without_auth() {
        assert!(webhook_dispatch_gate(&mk_webhook_linksys(None)));
    }

    #[test]
    fn webhook_dispatch_gate_open_with_shim_auth() {
        // Webhook auth is a deprecated shim field that is not used.
        // Dispatch gate is always open (NotRequired is buildable).
        let ls = mk_webhook_linksys(Some(serde_json::json!({
            "type": "bearer",
            "secretRef": {"name": "token"}
        })));
        assert!(webhook_dispatch_gate(&ls));
    }

    // ------------------------------------------------------------
    // Dispatch-gate composition: mirrors the exact expression used in
    // `runtime::store::dispatch_*` so these tests lock-in the invariant
    // "client is built iff auth.is_buildable() && tls_ready()".
    // ------------------------------------------------------------

    fn dispatch_gate(ls: &LinkSys) -> bool {
        basic_state(ls).is_buildable() && tls_ready(ls)
    }

    #[test]
    fn dispatch_gate_without_auth_is_open() {
        let ls = mk_redis_linksys(None);
        assert!(dispatch_gate(&ls));
    }

    #[test]
    fn dispatch_gate_blocks_pending_auth() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(None)));
        assert!(!dispatch_gate(&ls));
    }

    #[test]
    fn dispatch_gate_opens_with_resolved_auth() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("password", b"p")])))));
        assert!(dispatch_gate(&ls));
    }

    #[test]
    fn dispatch_gate_blocks_invalid_auth() {
        let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("wrong", b"x")])))));
        assert!(!dispatch_gate(&ls));
    }

    // ------------------------------------------------------------
    // require_basic_material helpers
    // ------------------------------------------------------------

    #[test]
    fn require_basic_resolved_returns_some() {
        let m = BasicAuthMaterial {
            username: Some("u".into()),
            password: "p".into(),
        };
        let state = CredentialState::Resolved(m);
        let got = require_basic_material(&state, "Redis").unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().username.as_deref(), Some("u"));
    }

    #[test]
    fn require_basic_pending_bails() {
        let state: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
        let err = require_basic_material(&state, "Redis").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Redis"), "msg should include provider: {msg}");
        assert!(msg.contains("has not been resolved yet"), "msg: {msg}");
    }

    #[test]
    fn require_basic_invalid_bails_with_reason() {
        let state: CredentialState<BasicAuthMaterial> =
            CredentialState::Invalid(InvalidReason::ParseFailed("missing key".into()));
        let err = require_basic_material(&state, "Etcd").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Etcd credential material is invalid"), "msg: {msg}");
        assert!(msg.contains("missing key"), "msg: {msg}");
    }

    #[test]
    fn require_basic_not_required_returns_none() {
        let state: CredentialState<BasicAuthMaterial> = CredentialState::NotRequired;
        let got = require_basic_material(&state, "Redis").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn dispatch_gate_blocks_tls_ref_without_secret() {
        let ls = LinkSys {
            metadata: ObjectMeta {
                name: Some("r".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: vec!["rediss://localhost:6379".into()],
                    auth: None,
                    db: None,
                    timeout: None,
                    pool: None,
                    retry: None,
                    topology: None,
                    tls: Some(CommonTlsConfig {
                        enabled: true,
                        // CA certificate refs declared but not yet resolved
                        outbound: OutboundTlsLocal {
                            validation: Some(OutboundTlsValidation {
                                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                    group: String::new(),
                                    kind: "Secret".into(),
                                    name: "tls".into(),
                                    namespace: None,
                                }]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        resolved_ca_certificates: None,
                        resolved_client_certificate: None,
                    }),
                    observability: None,
                }),
            },
            status: None,
        };
        assert!(!dispatch_gate(&ls));
    }

    // ------------------------------------------------------------
    // resolved_state — Kafka SASL variants
    // ------------------------------------------------------------

    fn mk_kafka_linksys(sasl: Option<edgion_resources::resources::link_sys::kafka::KafkaSaslConfig>) -> LinkSys {
        use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;
        LinkSys {
            metadata: kube::api::ObjectMeta {
                name: Some("kafka".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: edgion_resources::resources::link_sys::LinkSysSpec {
                config: SystemConfig::Kafka(KafkaClientConfig {
                    brokers: vec!["broker1:9092".to_string()],
                    sasl,
                    tls: None,
                    channel_size: None,
                    linger_ms: None,
                }),
            },
            status: None,
        }
    }

    #[test]
    fn resolved_state_kafka_no_sasl_is_not_required() {
        let ls = mk_kafka_linksys(None);
        assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    }

    #[test]
    fn resolved_state_kafka_sasl_pending_when_secret_missing() {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
        let ls = mk_kafka_linksys(Some(KafkaSaslConfig {
            username: "user".to_string(),
            password: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "kafka-secret".into(),
                    namespace: None,
                },
                secret: None,
            },
        }));
        assert!(matches!(resolved_state(&ls), CredentialState::Pending));
    }

    #[test]
    fn resolved_state_kafka_sasl_resolved_when_secret_present() {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
        let ls = mk_kafka_linksys(Some(KafkaSaslConfig {
            username: "user".to_string(),
            password: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "kafka-secret".into(),
                    namespace: None,
                },
                secret: Some(mk_secret(&[("password", b"kafka-pass")])),
            },
        }));
        assert!(matches!(
            resolved_state(&ls),
            CredentialState::Resolved(ResolvedCredential::Basic(_))
        ));
    }

    #[test]
    fn tls_ready_kafka_no_tls_is_true() {
        let ls = mk_kafka_linksys(None);
        assert!(tls_ready(&ls));
    }
}
