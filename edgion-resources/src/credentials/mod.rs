//! Shared credential parsing layer (used by both Controller and Gateway).
//!
//! Lives at the types layer so the Gateway can re-parse `Secret` bytes
//! attached to `LinkSys`/`EdgionAcme` specs on demand, without depending
//! on Controller-side wiring.
//!
//! Layout:
//! - [`materials`] — strongly-typed credential material structs
//! - [`resolver`] — [`SecretMaterialResolver`] trait + [`ResolveError`]
//! - [`resolvers`] — concrete implementations (Basic/Bearer/EsApiKey/AI/Acme/TLS)
//!
//! The Controller still owns the top-level owning-field credential pipeline
//! (cross-namespace `ReferenceGrant` enforcement, `SecretRefManager`
//! registration, status mapping). That orchestration lives under
//! `core::controller::conf_mgr::sync_runtime::resource_processor::credential`.

pub mod materials;
pub mod resolver;
pub mod resolvers;

pub use materials::{
    AcmeAccountMaterial, AiApiKeyMaterial, BasicAuthMaterial, BearerMaterial, EsApiKeyMaterial, TlsMaterial,
};
pub use resolver::{InvalidValueClass, ResolveError, SecretMaterialResolver};
pub use resolvers::{
    AcmeAccountResolver, AiApiKeyResolver, BasicAuthResolver, BearerResolver, EsApiKeyResolver, TlsCertsResolver,
    ACME_ACCOUNT_KEY, BEARER_TOKEN_KEY, TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY,
};
