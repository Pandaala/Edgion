//! `AuthSpec` / `TlsSpec` traits — bind a CRD auth/tls type to its resolver
//! choice and `SecretSlot`, so `resolve_for_spec` can dispatch generically.
//!
//! 新增 backend 4 步：
//!   1. 给新 *Auth 类型 impl AuthSpec（resolve() 内部挑 resolver + 调 resolve_one + 写回）
//!   2. （如有 TLS）TlsConfig 已 impl TlsSpec，新 backend 复用
//!   3. 在 SystemConfig enum 加变体
//!   4. resolve_for_spec 加一行 drive_standard(...)

use async_trait::async_trait;

use super::link_sys::resolve_one;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::context::HandlerContext;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use crate::types::credentials::resolvers::{
    BasicAuthResolver, BearerResolver, EsApiKeyResolver, HmacResolver, TlsCertsResolver, WebhookApiKeyResolver,
};
use crate::types::resources::common::{SecretSlot, TlsConfig};
use crate::types::resources::link_sys::elasticsearch::ElasticsearchAuth;
use crate::types::resources::link_sys::etcd::EtcdAuth;
use crate::types::resources::link_sys::redis::RedisAuth;
use crate::types::resources::link_sys::webhook::WebhookAuth;

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
/// (`tls.certs` absent or `secret_ref` absent — inline-only TLS); otherwise
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
// TlsSpec impl on TlsConfig — operates on TlsCerts.secret_ref + .secret directly
// (TlsCerts intentionally NOT migrated to SecretSlot, see plan T6 revision)
// ============================================================================

#[async_trait]
impl TlsSpec for TlsConfig {
    fn provider_name(&self) -> &'static str {
        "TLS"
    }
    async fn resolve(&mut self, ctx: &HandlerContext, owner: &ResourceRef) -> Option<ResolvedRefsError> {
        let certs = self.certs.as_mut()?;
        let secret_ref = certs.secret_ref.as_ref()?.clone();
        match resolve_one(ctx, owner, &secret_ref, &TlsCertsResolver).await {
            Ok(s) => {
                certs.secret = s;
                None
            }
            Err(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::{SecretObjectReference, SecretSlot};

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
}
