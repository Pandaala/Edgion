//! Concrete [`SecretMaterialResolver`](super::resolver::SecretMaterialResolver)
//! implementations keyed by auth type.

pub mod acme;
pub mod ai_api_key;
pub mod basic;
pub mod bearer;
pub mod dns_credential;
pub mod es_api_key;
pub mod hmac;
pub mod tls;
pub mod webhook_api_key;

pub use acme::{AcmeAccountResolver, ACME_ACCOUNT_KEY};
pub use ai_api_key::AiApiKeyResolver;
pub use basic::BasicAuthResolver;
pub use bearer::{BearerResolver, BEARER_TOKEN_KEY};
pub use dns_credential::{DnsCredentialMaterial, DnsCredentialResolver};
pub use es_api_key::EsApiKeyResolver;
pub use hmac::HmacResolver;
pub use tls::{TlsCertsResolver, TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
pub use webhook_api_key::WebhookApiKeyResolver;
