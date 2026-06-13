//! Certificate validation for EdgionTls — re-exports from `edgion-common`.
//!
//! The implementation (`validate_cert`, `CertValidationResult`, `CertValidationError`,
//! and all helpers) lives in `edgion_common::tls_validation` so the controller can call
//! them without depending on this crate.  This module is a thin re-export shim that keeps
//! existing import paths inside the gateway unchanged.
//!
//! A gateway-side caller helper that re-emits the advisory tracing warnings stripped from
//! the pure `validate_cert` (deprecated TLS version, reserved revocation/OCSP schema,
//! no-SAN CN fallback) will be added here in a later task.
pub use edgion_common::tls_validation::{validate_cert, CertValidationError, CertValidationResult};
