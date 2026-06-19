//! Controller-side credential resolution pipeline.
//!
//! The per-auth `SecretMaterialResolver` implementations and `Material`
//! structs live in [`edgion_resources::credentials`] so both the Controller
//! and the Gateway can consume them.
//!
//! This module only provides the orchestration layer: effective-namespace
//! computation, `ReferenceGrant` enforcement, `SecretRefManager` wiring and
//! Secret-lookup against the runtime caches.

pub mod pipeline;

pub use pipeline::{
    fetch_secret, resolve_credential, resolve_secret, FetchOutcome, InvalidCredentialError, PendingReason,
    ResolveOutcome,
};

// Re-export the credential parsing layer from its canonical location in
// edgion_resources; discoverable here for controller call sites.
pub use edgion_resources::credentials::{
    materials, resolver, resolvers, AcmeAccountMaterial, AcmeAccountResolver, BasicAuthMaterial, BasicAuthResolver,
    BearerMaterial, BearerResolver, EsApiKeyMaterial, EsApiKeyResolver, HmacMaterial, HmacResolver, ResolveError,
    SecretMaterialResolver, ACME_ACCOUNT_KEY,
};
