//! Provider-specific credential parsing over actual LinkSys field loans.
//! Reference defaults, target kinds and disabled TLS belong to Resources.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{resolve_bound_secret, AuthorizedSecret};
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::CredentialSecretInput;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::credentials::resolvers::{
    BasicAuthResolver, BearerResolver, EsApiKeyResolver, HmacResolver, TlsCertsResolver, WebhookApiKeyResolver,
};
use edgion_resources::resources::common::{SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchAuth;
use edgion_resources::resources::link_sys::etcd::EtcdAuth;
use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpBearerResolver};
use edgion_resources::resources::link_sys::redis::RedisAuth;
use edgion_resources::resources::link_sys::webhook::WebhookAuth;
use edgion_resources::resources::refs::link_sys_fields::{LinkSysCaSlot, LinkSysTlsReferences};
use edgion_resources::resources::refs::ReferenceToken;
use k8s_openapi::api::core::v1::Secret;

/// Select material parsing from the immutable actual provider auth variant.
pub trait AuthSpec: Send + Sync {
    fn slot(&self) -> &SecretSlot;
    fn slot_mut(&mut self) -> &mut SecretSlot;
    fn provider_name(&self) -> &'static str;
    fn resolve(
        &self,
        ctx: &HandlerContext,
        owner: &ResourceRef,
        token: ReferenceToken<'_>,
    ) -> Result<AuthorizedSecret, ResolvedRefsError>;
}

macro_rules! struct_auth {
    ($ty:ty, $slot:ident, $name:literal, $resolver:expr) => {
        impl AuthSpec for $ty {
            fn slot(&self) -> &SecretSlot {
                &self.$slot
            }
            fn slot_mut(&mut self) -> &mut SecretSlot {
                &mut self.$slot
            }
            fn provider_name(&self) -> &'static str {
                $name
            }
            fn resolve(
                &self,
                ctx: &HandlerContext,
                owner: &ResourceRef,
                token: ReferenceToken<'_>,
            ) -> Result<AuthorizedSecret, ResolvedRefsError> {
                resolve_bound_secret(ctx, owner, token, &$resolver)
            }
        }
    };
}
struct_auth!(RedisAuth, slot, "Redis", BasicAuthResolver);
struct_auth!(EtcdAuth, slot, "Etcd", BasicAuthResolver);
struct_auth!(OtlpAuth, slot, "OTLP", OtlpBearerResolver);
struct_auth!(KafkaSaslConfig, password, "Kafka", BasicAuthResolver);

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
    fn resolve(
        &self,
        ctx: &HandlerContext,
        owner: &ResourceRef,
        token: ReferenceToken<'_>,
    ) -> Result<AuthorizedSecret, ResolvedRefsError> {
        match self {
            Self::Basic { .. } => resolve_bound_secret(ctx, owner, token, &BasicAuthResolver),
            Self::ApiKey { .. } => resolve_bound_secret(ctx, owner, token, &EsApiKeyResolver),
            Self::Bearer { .. } => resolve_bound_secret(ctx, owner, token, &BearerResolver),
        }
    }
}

// This schema type is not a LinkSys config field and mints no owning token.
impl AuthSpec for WebhookAuth {
    fn slot(&self) -> &SecretSlot {
        match self {
            Self::Basic { slot } | Self::Bearer { slot } | Self::Hmac { slot } | Self::ApiKey { slot, .. } => slot,
        }
    }
    fn slot_mut(&mut self) -> &mut SecretSlot {
        match self {
            Self::Basic { slot } | Self::Bearer { slot } | Self::Hmac { slot } | Self::ApiKey { slot, .. } => slot,
        }
    }
    fn provider_name(&self) -> &'static str {
        "Webhook"
    }
    fn resolve(
        &self,
        ctx: &HandlerContext,
        owner: &ResourceRef,
        token: ReferenceToken<'_>,
    ) -> Result<AuthorizedSecret, ResolvedRefsError> {
        match self {
            Self::Basic { .. } => resolve_bound_secret(ctx, owner, token, &BasicAuthResolver),
            Self::Bearer { .. } => resolve_bound_secret(ctx, owner, token, &BearerResolver),
            Self::Hmac { .. } => resolve_bound_secret(ctx, owner, token, &HmacResolver),
            Self::ApiKey { .. } => resolve_bound_secret(ctx, owner, token, &WebhookApiKeyResolver),
        }
    }
}

#[derive(Default)]
pub(super) struct ResolvedTls {
    pub ca_certificates: Vec<Secret>,
    pub client_certificate: Option<Secret>,
    pub inputs: Vec<CredentialSecretInput>,
    pub error: Option<ResolvedRefsError>,
}
impl ResolvedTls {
    pub fn apply(self, tls: &mut TlsConfig) {
        tls.resolved_ca_certificates = None;
        tls.resolved_client_certificate = None;
        if self.error.is_none() {
            tls.resolved_ca_certificates = (!self.ca_certificates.is_empty()).then_some(self.ca_certificates);
            tls.resolved_client_certificate = self.client_certificate;
        }
    }
}

/// Preserve provider CA selection and publish one complete enabled TLS block.
pub(super) fn resolve_tls(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: LinkSysTlsReferences<'_>,
) -> ResolvedTls {
    let mut result = ResolvedTls::default();
    for slot in references.ca_certificates() {
        let LinkSysCaSlot::Secret(token) = slot else {
            continue;
        };
        match resolve_bound_secret(ctx, owner, token, &TlsCertsResolver) {
            Ok(authorized) => {
                match super::link_sys::secret_input("tls-ca", token, &authorized) {
                    Ok(input) => result.inputs.push(input),
                    Err(error) => {
                        result.error.get_or_insert(error);
                    }
                }
                result.ca_certificates.push(authorized.secret);
            }
            Err(error) => {
                result.error.get_or_insert(error);
            }
        }
    }
    if let Some(token) = references.client_certificate() {
        match resolve_bound_secret(ctx, owner, token, &TlsCertsResolver) {
            Ok(authorized) => {
                match super::link_sys::secret_input("tls-client-certificate", token, &authorized) {
                    Ok(input) => result.inputs.push(input),
                    Err(error) => {
                        result.error.get_or_insert(error);
                    }
                }
                result.client_certificate = Some(authorized.secret);
            }
            Err(error) => {
                result.error.get_or_insert(error);
            }
        }
    }
    result
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/auth_spec_test.rs"]
mod tests;
