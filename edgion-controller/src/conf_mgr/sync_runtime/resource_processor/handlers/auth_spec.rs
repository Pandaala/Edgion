//! `AuthSpec` / `TlsSpec` traits — bind a CRD auth/tls type to its resolver
//! choice and `SecretSlot`, so `resolve_for_spec` can dispatch generically.
//!
//! Adding a new backend takes 4 steps:
//!   1. impl AuthSpec for the new *Auth type (resolve() picks the resolver,
//!      calls resolve_one, and writes the result back).
//!   2. If TLS is involved, TlsConfig already impls TlsSpec; the new backend
//!      reuses it.
//!   3. Add a variant to the SystemConfig enum.
//!   4. Add one drive_standard(...) line in resolve_for_spec.

use async_trait::async_trait;

use super::link_sys::resolve_one;
use crate::conf_mgr::sync_runtime::resource_processor::context::HandlerContext;
use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use edgion_resources::credentials::resolvers::{
    BasicAuthResolver, BearerResolver, EsApiKeyResolver, HmacResolver, TlsCertsResolver, WebhookApiKeyResolver,
};
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchAuth;
use edgion_resources::resources::link_sys::etcd::EtcdAuth;
use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
use edgion_resources::resources::link_sys::otlp::{validate_otlp_secret_ref, OtlpAuth, OtlpBearerResolver};
use edgion_resources::resources::link_sys::redis::RedisAuth;
use edgion_resources::resources::link_sys::webhook::WebhookAuth;

/// Bind an auth-bearing CRD type to its resolver and resolution path.
///
/// `resolve()` must:
/// 1. Pick the appropriate resolver (per variant for enum forms).
/// 2. Call `resolve_one(ctx, owner, &slot.secret_ref, &Resolver)`.
/// 3. On success, write the resolved `Secret` back into `slot.secret`.
/// 4. Return `Some(error)` on failure, `None` on success.
#[async_trait]
pub trait AuthSpec: Send {
    fn slot(&self) -> &SecretSlot;
    fn slot_mut(&mut self) -> &mut SecretSlot;
    fn provider_name(&self) -> &'static str;

    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError>;
}

/// Bind a TLS-bearing CRD type to its resolution path.
///
/// `resolve()` returns `None` when there is no TLS material to resolve
/// (`tls.clientCertificateRef` absent — inline or System-only TLS); otherwise
/// returns `Some(err)` on failure / `None` on success.
#[async_trait]
pub trait TlsSpec: Send {
    fn provider_name(&self) -> &'static str;

    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError>;
}

// ============================================================================
// AuthSpec impls — struct form (Redis / Etcd)
// ============================================================================

#[async_trait]
impl AuthSpec for RedisAuth {
    fn slot(&self) -> &SecretSlot {
        &self.slot
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        &mut self.slot
    }
    fn provider_name(&self) -> &'static str {
        "Redis"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        match resolve_one(ctx, owner, &self.slot.secret_ref, &BasicAuthResolver).await {
            Ok(s) => {
                self.slot.secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

#[async_trait]
impl AuthSpec for EtcdAuth {
    fn slot(&self) -> &SecretSlot {
        &self.slot
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        &mut self.slot
    }
    fn provider_name(&self) -> &'static str {
        "Etcd"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        match resolve_one(ctx, owner, &self.slot.secret_ref, &BasicAuthResolver).await {
            Ok(s) => {
                self.slot.secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

#[async_trait]
impl AuthSpec for OtlpAuth {
    fn slot(&self) -> &SecretSlot {
        &self.slot
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        &mut self.slot
    }
    fn provider_name(&self) -> &'static str {
        "OTLP"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        if let Some(error) = otlp_reference_error(&self.slot.secret_ref, "auth.secretRef") {
            self.slot.secret = None;
            return Some(error);
        }
        match resolve_one(ctx, owner, &self.slot.secret_ref, &OtlpBearerResolver).await {
            Ok(secret) => {
                self.slot.secret = secret;
                None
            }
            Err(error) => Some(error),
        }
    }
}

fn otlp_reference_error(reference: &SecretObjectReference, field: &'static str) -> Option<ResolvedRefsError> {
    validate_otlp_secret_ref(reference)
        .err()
        .map(|_| ResolvedRefsError::InvalidKind {
            // Keep malformed identity fields out of status and diagnostic messages.
            kind: "unsupported OTLP Secret reference".into(),
            name: field.into(),
        })
}

// ============================================================================
// AuthSpec impls — enum form (Webhook / Elasticsearch)
// ============================================================================

#[async_trait]
impl AuthSpec for WebhookAuth {
    fn slot(&self) -> &SecretSlot {
        match self {
            Self::Basic { slot } | Self::Bearer { slot } | Self::Hmac { slot } => slot,
            Self::ApiKey { slot, .. } => slot,
        }
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        match self {
            Self::Basic { slot } | Self::Bearer { slot } | Self::Hmac { slot } => slot,
            Self::ApiKey { slot, .. } => slot,
        }
    }
    fn provider_name(&self) -> &'static str {
        "Webhook"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        // Pick resolver by variant; clone the secret_ref to release the borrow before write-back.
        let secret_ref = self.slot().secret_ref.clone();
        let result = match self {
            Self::Basic { .. } => resolve_one(ctx, owner, &secret_ref, &BasicAuthResolver).await,
            Self::Bearer { .. } => resolve_one(ctx, owner, &secret_ref, &BearerResolver).await,
            Self::Hmac { .. } => resolve_one(ctx, owner, &secret_ref, &HmacResolver).await,
            Self::ApiKey { .. } => resolve_one(ctx, owner, &secret_ref, &WebhookApiKeyResolver).await,
        };
        match result {
            Ok(s) => {
                self.slot_mut().secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

#[async_trait]
impl AuthSpec for ElasticsearchAuth {
    fn slot(&self) -> &SecretSlot {
        match self {
            Self::Basic { slot } | Self::ApiKey { slot } | Self::Bearer { slot } => slot,
        }
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        match self {
            Self::Basic { slot } | Self::ApiKey { slot } | Self::Bearer { slot } => slot,
        }
    }
    fn provider_name(&self) -> &'static str {
        "Elasticsearch"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        let secret_ref = self.slot().secret_ref.clone();
        let result = match self {
            Self::Basic { .. } => resolve_one(ctx, owner, &secret_ref, &BasicAuthResolver).await,
            Self::ApiKey { .. } => resolve_one(ctx, owner, &secret_ref, &EsApiKeyResolver).await,
            Self::Bearer { .. } => resolve_one(ctx, owner, &secret_ref, &BearerResolver).await,
        };
        match result {
            Ok(s) => {
                self.slot_mut().secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

// ============================================================================
// AuthSpec impl — Kafka SASL/PLAIN
// ============================================================================

#[async_trait]
impl AuthSpec for KafkaSaslConfig {
    fn slot(&self) -> &SecretSlot {
        &self.password
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        &mut self.password
    }
    fn provider_name(&self) -> &'static str {
        "Kafka"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        match resolve_one(ctx, owner, &self.password.secret_ref, &BasicAuthResolver).await {
            Ok(s) => {
                self.password.secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

// ============================================================================
// TlsSpec impl on TlsConfig — resolves outbound CA cert refs and client cert
// ref into resolved_ca_certificates / resolved_client_certificate.
// ============================================================================

#[async_trait]
impl TlsSpec for TlsConfig {
    fn provider_name(&self) -> &'static str {
        "TLS"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        resolve_tls(self, ctx, owner, false).await
    }
}

/// OTLP retains the shared TLS material loader, with strict identity validation
/// before each lookup. Other LinkSys providers keep their existing policy.
pub(super) struct OtlpTlsSpec<'a>(pub &'a mut TlsConfig);

#[async_trait]
impl TlsSpec for OtlpTlsSpec<'_> {
    fn provider_name(&self) -> &'static str {
        "OTLP TLS"
    }

    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        resolve_tls(self.0, ctx, owner, true).await
    }
}

async fn resolve_tls(
    tls: &mut TlsConfig,
    ctx: &HandlerContext,
    owner: &ResourceRef,
    strict_otlp_refs: bool,
) -> Option<ResolvedRefsError> {
    // Reset runtime-resolved fields first to avoid stale values on update.
    tls.resolved_ca_certificates = None;
    tls.resolved_client_certificate = None;
    if !tls.enabled {
        return None;
    }

    // Resolve CA certificate refs (type-validated via TlsCertsResolver).
    // Note: ca_refs::resolve_ca_certificate_refs is intentionally NOT used here
    // because TlsSpec.resolve() must return InvalidSecretType on a bad CA Secret
    // type (e.g. kubernetes.io/ssh-auth). The ca_refs helper skips type
    // validation; resolve_one + TlsCertsResolver enforces the whitelist.
    let refs = tls
        .outbound
        .validation
        .as_ref()
        .and_then(|v| v.ca_certificate_refs.as_deref())
        .unwrap_or_default();
    let mut first_error = None;
    let mut cas = Vec::new();
    if !refs.is_empty() {
        for ca_ref in refs {
            let sor = SecretObjectReference {
                group: Some(ca_ref.group.clone()),
                kind: Some(ca_ref.kind.clone()),
                name: ca_ref.name.clone(),
                namespace: ca_ref.namespace.clone(),
            };
            if strict_otlp_refs {
                if let Some(error) = otlp_reference_error(&sor, "tls.validation.caCertificateRefs") {
                    first_error.get_or_insert(error);
                    continue;
                }
            }
            // Other providers retain their existing non-Secret skip policy.
            if ca_ref.kind != "Secret" {
                continue;
            }
            match resolve_one(ctx, owner, &sor, &TlsCertsResolver).await {
                Ok(Some(s)) => cas.push(s),
                Ok(None) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
    }

    // Resolve client certificate ref (mTLS identity).
    let mut client_certificate = None;
    if let Some(client_ref) = tls.outbound.client_certificate_ref.as_ref() {
        let result = match strict_otlp_refs
            .then(|| otlp_reference_error(client_ref, "tls.clientCertificateRef"))
            .flatten()
        {
            Some(error) => Err(error),
            None => resolve_one(ctx, owner, client_ref, &TlsCertsResolver).await,
        };
        match result {
            Ok(secret) => client_certificate = secret,
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    // Register every dependency, but publish only one complete TLS generation.
    if first_error.is_none() {
        tls.resolved_ca_certificates = (!cas.is_empty()).then_some(cas);
        tls.resolved_client_certificate = client_certificate;
    }
    first_error
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};

    fn slot(name: &str) -> SecretSlot {
        SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: name.into(),
                namespace: None,
            },
            secret: None,
        }
    }

    #[test]
    fn redis_auth_provider_name() {
        let a = RedisAuth { slot: slot("r") };
        assert_eq!(<RedisAuth as AuthSpec>::provider_name(&a), "Redis");
        assert_eq!(<RedisAuth as AuthSpec>::slot(&a).secret_ref.name, "r");
    }

    #[test]
    fn webhook_auth_apikey_slot_view() {
        let a = WebhookAuth::ApiKey {
            slot: slot("w"),
            r#in: "header".into(),
            header_name: Some("X-API-Key".into()),
            query_param: None,
        };
        assert_eq!(<WebhookAuth as AuthSpec>::slot(&a).secret_ref.name, "w");
    }

    #[test]
    fn es_auth_basic_slot_view() {
        let a = ElasticsearchAuth::Basic { slot: slot("es") };
        assert_eq!(<ElasticsearchAuth as AuthSpec>::slot(&a).secret_ref.name, "es");
    }

    #[test]
    fn kafka_sasl_provider_name_and_slot() {
        let a = KafkaSaslConfig {
            username: "user".to_string(),
            password: slot("kafka-secret"),
        };
        assert_eq!(<KafkaSaslConfig as AuthSpec>::provider_name(&a), "Kafka");
        assert_eq!(<KafkaSaslConfig as AuthSpec>::slot(&a).secret_ref.name, "kafka-secret");
    }
}
