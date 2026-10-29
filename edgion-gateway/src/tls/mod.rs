pub(crate) mod curve_policy;
pub(crate) mod downstream_policy;
pub mod fips_host;
pub mod fips_profile;
pub mod runtime;
pub mod store;
pub mod validation;

#[cfg(feature = "boringssl")]
pub mod boringssl;

pub use store::{create_tls_handler, get_global_tls_store};
pub use validation::{validate_cert, CertValidationError, CertValidationResult};
// Re-export gateway listener TLS matcher helpers from the runtime matching layer.
pub use crate::runtime::matching::{
    get_gateway_tls_matcher, match_gateway_tls, rebuild_gateway_tls_matcher, GatewayTlsEntry, GatewayTlsMatcher,
};
