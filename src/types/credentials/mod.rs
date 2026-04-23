//! Shared credential parsing layer (used by both Controller and Gateway).
//!
//! Historically this module lived under the Controller's credential
//! pipeline. It was moved to the types layer so that the Gateway can
//! re-parse `Secret` bytes attached to `LinkSys`/`EdgionAcme` specs and
//! derive its own `Material` values on demand, without depending on any
//! Controller-side wiring.
//!
//! Layout:
//! - [`materials`] — strongly-typed credential material structs
//! - [`resolver`] — [`SecretMaterialResolver`] trait + [`ResolveError`]
//! - [`resolvers`] — concrete implementations (Basic/Bearer/Hmac/EsApiKey/Acme)
//!
//! The Controller still owns the top-level `resolve_credential` pipeline
//! (cross-namespace `ReferenceGrant` enforcement, `SecretRefManager`
//! registration, status mapping). That orchestration lives under
//! `core::controller::conf_mgr::sync_runtime::resource_processor::credential`.

pub mod materials;
pub mod resolver;
pub mod resolvers;

pub use materials::{
    AcmeAccountMaterial, BasicAuthMaterial, BearerMaterial, EsApiKeyMaterial, HmacMaterial, TlsMaterial,
};
pub use resolver::{ResolveError, SecretMaterialResolver};
pub use resolvers::{
    AcmeAccountResolver, BasicAuthResolver, BearerResolver, EsApiKeyResolver, HmacResolver, TlsCertsResolver,
    ACME_ACCOUNT_KEY, TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY,
};
