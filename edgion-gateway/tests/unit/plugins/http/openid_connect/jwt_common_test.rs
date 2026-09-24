use super::super::error::{map_jwt_decode_error_typed, OidcVerifyError};
use super::{jwk_matches_alg, resolve_algorithm_policy, select_jwk, JwkSelectError};
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
    assert_eq!(map_jwt_decode_error_typed(err), OidcVerifyError::InvalidFormat);
}
