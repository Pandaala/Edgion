//! JWT/JWKS helpers for the gateway OpenID Connect plugin.

use super::error::{AuthFailure, OidcVerifyError};
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, Header};
use std::str::FromStr;

pub(super) type VerifyResult<T> = Result<T, OidcVerifyError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwkSelectError {
    KidNotFound,
    NoSuitableKey,
}

pub fn default_allowed_algs() -> Vec<Algorithm> {
    vec![
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::ES256,
        Algorithm::ES384,
    ]
}

pub fn resolve_algorithm_policy(
    token_signing_alg: Option<&str>,
    allowed_signing_algs: Option<&[String]>,
) -> VerifyResult<(Option<Algorithm>, Vec<Algorithm>)> {
    let expected = if let Some(configured) = token_signing_alg {
        Some(Algorithm::from_str(configured).map_err(|_| OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm))?)
    } else {
        None
    };

    let allowed = if let Some(configured) = allowed_signing_algs {
        let mut out = Vec::with_capacity(configured.len());
        for alg in configured {
            let parsed = Algorithm::from_str(alg).map_err(|_| OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm))?;
            out.push(parsed);
        }
        out
    } else {
        default_allowed_algs()
    };

    if allowed.is_empty() {
        return Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm));
    }

    if let Some(exp) = expected {
        if !allowed.contains(&exp) {
            return Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm));
        }
    }

    Ok((expected, allowed))
}

pub fn validate_token_alg(
    token_alg: Algorithm,
    expected_alg: Option<Algorithm>,
    allowed_algs: &[Algorithm],
) -> VerifyResult<()> {
    if let Some(expected) = expected_alg {
        if token_alg != expected {
            return Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm));
        }
    }

    if !allowed_algs.contains(&token_alg) {
        return Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm));
    }

    Ok(())
}

pub fn jwk_matches_alg(jwk: &Jwk, alg: Algorithm) -> bool {
    if let Some(key_alg) = jwk.common.key_algorithm {
        let key_alg_str = key_alg.to_string();
        let Ok(key_alg_as_alg) = Algorithm::from_str(&key_alg_str) else {
            return false;
        };
        if key_alg_as_alg != alg {
            return false;
        }
    }

    match (&jwk.algorithm, alg) {
        (AlgorithmParameters::RSA(_), Algorithm::RS256)
        | (AlgorithmParameters::RSA(_), Algorithm::RS384)
        | (AlgorithmParameters::RSA(_), Algorithm::RS512)
        | (AlgorithmParameters::RSA(_), Algorithm::PS256)
        | (AlgorithmParameters::RSA(_), Algorithm::PS384)
        | (AlgorithmParameters::RSA(_), Algorithm::PS512) => true,
        (AlgorithmParameters::EllipticCurve(params), Algorithm::ES256) => params.curve == EllipticCurve::P256,
        (AlgorithmParameters::EllipticCurve(params), Algorithm::ES384) => params.curve == EllipticCurve::P384,
        (AlgorithmParameters::OctetKey(_), Algorithm::HS256)
        | (AlgorithmParameters::OctetKey(_), Algorithm::HS384)
        | (AlgorithmParameters::OctetKey(_), Algorithm::HS512) => true,
        (AlgorithmParameters::OctetKeyPair(params), Algorithm::EdDSA) => params.curve == EllipticCurve::Ed25519,
        _ => false,
    }
}

pub fn select_jwk<'a>(header: &Header, jwks: &'a JwkSet, alg: Algorithm) -> Result<&'a Jwk, JwkSelectError> {
    if let Some(kid) = header.kid.as_deref() {
        let Some(jwk) = jwks.find(kid) else {
            return Err(JwkSelectError::KidNotFound);
        };
        if jwk_matches_alg(jwk, alg) {
            return Ok(jwk);
        }
        return Err(JwkSelectError::NoSuitableKey);
    }

    jwks.keys
        .iter()
        .find(|j| jwk_matches_alg(j, alg))
        .ok_or(JwkSelectError::NoSuitableKey)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/openid_connect/jwt_common_test.rs"]
mod tests;
