//! Concrete [`SecretMaterialResolver`](super::resolver::SecretMaterialResolver)
//! implementations keyed by auth type.

pub mod acme;
pub mod basic;
pub mod bearer;
pub mod dns_credential;
pub mod es_api_key;
pub mod hmac;
pub mod tls;

pub use acme::{AcmeAccountResolver, ACME_ACCOUNT_KEY};
pub use basic::BasicAuthResolver;
pub use bearer::BearerResolver;
pub use dns_credential::{DnsCredentialMaterial, DnsCredentialResolver};
pub use es_api_key::EsApiKeyResolver;
pub use hmac::HmacResolver;
pub use tls::{TlsCertsResolver, TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
