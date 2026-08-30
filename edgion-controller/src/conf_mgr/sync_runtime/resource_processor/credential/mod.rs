//! Controller-side credential resolution pipeline.
//!
//! The per-auth `SecretMaterialResolver` implementations and `Material`
//! structs live in [`edgion_resources::credentials`] so both the Controller
//! and the Gateway can consume them.
//!
//! This module only provides the orchestration layer: effective-namespace
//! computation, `ReferenceGrant` enforcement, `SecretRefManager` wiring and
//! Secret-lookup against the runtime caches.

pub mod ca_refs;
pub mod pipeline;
pub mod request_secrets;

pub use ca_refs::{resolve_ca_certificate_refs, CaFetchStrategy};
pub use pipeline::{
    fetch_configmap, fetch_secret, resolve_credential, resolve_secret, FetchOutcome, InvalidCredentialError,
    PendingReason, ResolveOutcome,
};
pub use request_secrets::{request_secret_errors_readonly, resolve_request_secrets};

// Re-export the credential parsing layer from its canonical location in
// edgion_resources; discoverable here for controller call sites.
pub use edgion_resources::credentials::{
    materials, resolver, resolvers, AcmeAccountMaterial, AcmeAccountResolver, BasicAuthMaterial, BasicAuthResolver,
    BearerMaterial, BearerResolver, EsApiKeyMaterial, EsApiKeyResolver, HmacMaterial, HmacResolver, ResolveError,
    SecretMaterialResolver, ACME_ACCOUNT_KEY,
};
