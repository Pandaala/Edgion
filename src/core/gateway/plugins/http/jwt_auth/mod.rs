//! JWT Authentication plugin (HTTP).
//!
//! See `docs/superpowers/specs/2026-04-25-P0-08-jwt-auth-plugin-split-design.md`
//! for module split rationale.

mod credentials;
mod plugin;
mod request_stage;
mod verify;

#[cfg(test)]
mod tests;

pub use plugin::JwtAuth;

use crate::types::resources::edgion_plugins::JwtAlgorithm;
use jsonwebtoken::Algorithm;

/// Unit error: failure kind already lives in the `PluginLog` tag pushed at the
/// failure site (e.g. `Deny token expired`, `Deny bad algorithm`). The caller
/// always discards the error value, so carrying token / claim content in a
/// `Box<dyn Error>` would only risk a future leak (see
/// `skills/03-coding/01-log-safety.md`, Iron rule 1).
pub(crate) type JwtAuthResult<T> = Result<T, ()>;

/// Convert JwtAlgorithm domain type to jsonwebtoken::Algorithm.
pub(crate) fn to_jwt_algorithm(alg: JwtAlgorithm) -> Algorithm {
    match alg {
        JwtAlgorithm::HS256 => Algorithm::HS256,
        JwtAlgorithm::HS384 => Algorithm::HS384,
        JwtAlgorithm::HS512 => Algorithm::HS512,
        JwtAlgorithm::RS256 => Algorithm::RS256,
        JwtAlgorithm::RS384 => Algorithm::RS384,
        JwtAlgorithm::RS512 => Algorithm::RS512,
        JwtAlgorithm::ES256 => Algorithm::ES256,
        JwtAlgorithm::ES384 => Algorithm::ES384,
    }
}

/// Whether the algorithm is symmetric (HS*).
pub(crate) fn is_symmetric(alg: JwtAlgorithm) -> bool {
    matches!(alg, JwtAlgorithm::HS256 | JwtAlgorithm::HS384 | JwtAlgorithm::HS512)
}
