pub mod backend_san;
pub mod cert;
pub mod mtls;
pub mod outbound_client;
pub mod outbound_san_verifier;

pub use edgion_common::tls_validation::{validate_cert, CertValidationError, CertValidationResult};
pub use mtls::{validate_cn_whitelist, validate_san_whitelist};
pub use outbound_client::build_verified_outbound_tls;
pub use outbound_san_verifier::{outbound_san_mismatch_reason, OutboundSanVerifier, SanMismatchError};
