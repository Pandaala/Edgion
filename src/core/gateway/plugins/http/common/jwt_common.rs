//! Shared JWT/JWKS helpers for auth plugins.

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, Header};
use std::str::FromStr;

pub type VerifyResult<T> = Result<T, (u16, String)>;

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
        Some(Algorithm::from_str(configured).map_err(|_| (401, format!("Invalid tokenSigningAlg: {}", configured)))?)
    } else {
        None
    };

    let allowed = if let Some(configured) = allowed_signing_algs {
        let mut out = Vec::with_capacity(configured.len());
        for alg in configured {
            let parsed =
                Algorithm::from_str(alg).map_err(|_| (401, format!("Invalid allowedSigningAlgs entry: {}", alg)))?;
            out.push(parsed);
        }
        out
    } else {
        default_allowed_algs()
    };

    if allowed.is_empty() {
        return Err((401, "allowedSigningAlgs cannot be empty".to_string()));
    }

    if let Some(exp) = expected {
        if !allowed.contains(&exp) {
            return Err((
                401,
                "tokenSigningAlg must be included in allowedSigningAlgs".to_string(),
            ));
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
            return Err((
                401,
                format!("Invalid token algorithm: expected {:?}, got {:?}", expected, token_alg),
            ));
        }
    }

    if !allowed_algs.contains(&token_alg) {
        return Err((401, format!("Token algorithm {:?} is not allowed", token_alg)));
    }

    Ok(())
}

pub fn map_jwt_decode_error(err: jsonwebtoken::errors::Error) -> (u16, String) {
    match err.kind() {
        ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Json(_) | ErrorKind::Utf8(_) => {
            (401, "Invalid token format".to_string())
        }
        ErrorKind::ExpiredSignature => (401, "Token expired".to_string()),
        ErrorKind::InvalidIssuer => (401, "Invalid token issuer".to_string()),
        ErrorKind::InvalidAudience => (401, "Invalid token audience".to_string()),
        ErrorKind::ImmatureSignature => (401, "Token not yet valid".to_string()),
        ErrorKind::InvalidSignature => (401, "Invalid token signature".to_string()),
        ErrorKind::InvalidAlgorithm => (401, "Invalid token algorithm".to_string()),
        _ => {
            tracing::debug!("JWT verification failed: {}", err);
            (401, "Invalid token".to_string())
        }
    }
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

pub fn select_jwk(header: &Header, jwks: &JwkSet, alg: Algorithm) -> Result<Jwk, JwkSelectError> {
    if let Some(kid) = header.kid.as_deref() {
        let Some(jwk) = jwks.find(kid) else {
            return Err(JwkSelectError::KidNotFound);
        };
        if jwk_matches_alg(jwk, alg) {
            return Ok(jwk.clone());
        }
        return Err(JwkSelectError::NoSuitableKey);
    }

    jwks.keys
        .iter()
        .find(|j| jwk_matches_alg(j, alg))
        .cloned()
        .ok_or(JwkSelectError::NoSuitableKey)
}

#[cfg(test)]
mod tests {
    use super::{jwk_matches_alg, map_jwt_decode_error, resolve_algorithm_policy, select_jwk, JwkSelectError};
    use jsonwebtoken::errors::ErrorKind;
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, Jwk, JwkSet, RSAKeyParameters,
    };
    use jsonwebtoken::{Algorithm, Header};

    #[test]
    fn test_resolve_algorithm_policy_defaults() {
        let (expected, allowed) = resolve_algorithm_policy(None, None).expect("resolve");
        assert!(expected.is_none());
        assert!(allowed.contains(&Algorithm::RS256));
    }

    #[test]
    fn test_select_jwk_kid_miss() {
        let header = Header {
            kid: Some("missing".to_string()),
            alg: Algorithm::RS256,
            ..Header::default()
        };
        let jwks = JwkSet { keys: vec![] };
        assert_eq!(
            select_jwk(&header, &jwks, Algorithm::RS256),
            Err(JwkSelectError::KidNotFound)
        );
    }

    #[test]
    fn test_jwk_matches_alg_rsa() {
        let jwk = Jwk {
            common: CommonParameters::default(),
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                key_type: Default::default(),
                n: "n".to_string(),
                e: "e".to_string(),
            }),
        };
        assert!(jwk_matches_alg(&jwk, Algorithm::RS256));
        assert!(!jwk_matches_alg(&jwk, Algorithm::ES256));
    }

    #[test]
    fn test_jwk_matches_alg_ec_curve() {
        let jwk = Jwk {
            common: CommonParameters::default(),
            algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
                key_type: Default::default(),
                curve: EllipticCurve::P256,
                x: "x".to_string(),
                y: "y".to_string(),
            }),
        };
        assert!(jwk_matches_alg(&jwk, Algorithm::ES256));
        assert!(!jwk_matches_alg(&jwk, Algorithm::ES384));
    }

    #[test]
    fn test_map_jwt_decode_error_format() {
        let err = jsonwebtoken::errors::Error::from(ErrorKind::InvalidToken);
        assert_eq!(map_jwt_decode_error(err), (401, "Invalid token format".to_string()));
    }
}
