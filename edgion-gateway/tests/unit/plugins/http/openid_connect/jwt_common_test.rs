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

#[test]
fn signing_policy_preserves_exact_defaults_and_explicit_opt_ins() {
    use super::validate_token_alg;

    let (_, defaults) = resolve_algorithm_policy(None, None).unwrap();
    assert_eq!(
        defaults,
        vec![
            Algorithm::RS256,
            Algorithm::RS384,
            Algorithm::RS512,
            Algorithm::ES256,
            Algorithm::ES384
        ]
    );
    for (name, algorithm) in [
        ("HS256", Algorithm::HS256),
        ("HS384", Algorithm::HS384),
        ("HS512", Algorithm::HS512),
        ("PS256", Algorithm::PS256),
        ("PS384", Algorithm::PS384),
        ("PS512", Algorithm::PS512),
        ("EdDSA", Algorithm::EdDSA),
    ] {
        assert!(validate_token_alg(algorithm, None, &defaults).is_err());
        let allowed = vec![name.to_string()];
        let (expected, resolved) = resolve_algorithm_policy(Some(name), Some(&allowed)).unwrap();
        assert_eq!(expected, Some(algorithm));
        assert_eq!(resolved, vec![algorithm]);
        assert!(validate_token_alg(algorithm, expected, &resolved).is_ok());
        assert!(validate_token_alg(Algorithm::RS256, expected, &resolved).is_err());
    }
    assert!(validate_token_alg(Algorithm::RS384, Some(Algorithm::RS256), &defaults).is_err());
}

#[test]
fn signing_policy_rejects_invalid_configuration_without_error_payloads() {
    use super::super::error::AuthFailure;

    for (expected, allowed) in [
        (Some("hs256"), None),
        (Some("unknown"), None),
        (None, Some(vec!["rs256".to_string()])),
        (None, Some(vec!["unknown".to_string()])),
        (None, Some(vec![])),
        (Some("HS256"), None),
        (Some("RS256"), Some(vec!["ES256".to_string()])),
    ] {
        assert_eq!(
            resolve_algorithm_policy(expected, allowed.as_deref()),
            Err(AuthFailure::InvalidAlgorithm)
        );
    }
}

#[test]
fn jwk_algorithm_metadata_matches_locked_conversion_for_all_variants() {
    use jsonwebtoken::jwk::KeyAlgorithm;
    use serde_json::json;
    use std::str::FromStr;

    let algorithms = [
        Algorithm::HS256,
        Algorithm::HS384,
        Algorithm::HS512,
        Algorithm::ES256,
        Algorithm::ES384,
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::PS256,
        Algorithm::PS384,
        Algorithm::PS512,
        Algorithm::EdDSA,
    ];
    let key_algorithms = [
        KeyAlgorithm::HS256,
        KeyAlgorithm::HS384,
        KeyAlgorithm::HS512,
        KeyAlgorithm::ES256,
        KeyAlgorithm::ES384,
        KeyAlgorithm::RS256,
        KeyAlgorithm::RS384,
        KeyAlgorithm::RS512,
        KeyAlgorithm::PS256,
        KeyAlgorithm::PS384,
        KeyAlgorithm::PS512,
        KeyAlgorithm::EdDSA,
        KeyAlgorithm::RSA1_5,
        KeyAlgorithm::RSA_OAEP,
        KeyAlgorithm::RSA_OAEP_256,
    ];
    for fixture in [
        json!({"kty": "RSA", "n": "AQ", "e": "AQ"}),
        json!({"kty": "EC", "crv": "P-256", "x": "AQ", "y": "AQ"}),
        json!({"kty": "EC", "crv": "P-384", "x": "AQ", "y": "AQ"}),
        json!({"kty": "oct", "k": "AQ"}),
        json!({"kty": "OKP", "crv": "Ed25519", "x": "AQ"}),
    ] {
        let mut jwk: Jwk = serde_json::from_value(fixture).unwrap();
        for algorithm in algorithms {
            jwk.common.key_algorithm = None;
            let family_matches = jwk_matches_alg(&jwk, algorithm);
            for key_algorithm in key_algorithms {
                jwk.common.key_algorithm = Some(key_algorithm);
                // Differential oracle: the locked dependency's former text conversion.
                let metadata_matches = Algorithm::from_str(&key_algorithm.to_string()) == Ok(algorithm);
                assert_eq!(
                    jwk_matches_alg(&jwk, algorithm),
                    family_matches && metadata_matches,
                    "key algorithm {key_algorithm:?}, token algorithm {algorithm:?}"
                );
            }
        }
    }
}

#[test]
fn old_ring_signatures_verify_through_oidc_algorithm_and_jwk_policy() {
    use super::validate_token_alg;
    use base64::Engine;
    use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
    use serde_json::Value;
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/jwt_dependency_signatures.json")).unwrap();
    let mut checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["alg"].as_str().unwrap();
        let token = case["token"].as_str().unwrap();
        let header = decode_header(token).unwrap();
        let allowed = vec![name.to_owned()];
        let (expected, allowed) = resolve_algorithm_policy(Some(name), Some(&allowed)).unwrap();
        validate_token_alg(header.alg, expected, &allowed).unwrap();
        let jwks = JwkSet {
            keys: vec![serde_json::from_value(case["jwk"].clone()).unwrap()],
        };
        let jwk = select_jwk(&header, &jwks, header.alg).unwrap();
        let key = DecodingKey::from_jwk(jwk).unwrap();
        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_issuer(&["https://jwt-fixture.example"]);
        validation.set_audience(&["fixture-client"]);
        let verified = decode::<Value>(token, &key, &validation).unwrap();
        assert_eq!(verified.claims["sub"], "fixture-user");
        let (message, signature) = token.rsplit_once('.').unwrap();
        let mut signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .unwrap();
        signature[0] ^= 1;
        let tampered = format!(
            "{message}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        let error = decode::<Value>(&tampered, &key, &validation).unwrap_err();
        assert!(!matches!(
            map_jwt_decode_error_typed(error),
            OidcVerifyError::InvalidFormat
        ));
        checked += 1;
    }
    assert_eq!(checked, 12);
}

#[test]
fn unknown_jwk_type_with_matching_kid_cannot_select_another_key() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/jwt_dependency_signatures.json")).unwrap();
    let rsa = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["alg"] == "RS256")
        .unwrap();
    let future: Jwk =
        serde_json::from_value(serde_json::json!({"kty":"FutureSigningKey","alg":"RS256","kid":"unknown-key"}))
            .unwrap();
    assert!(!jwk_matches_alg(&future, Algorithm::RS256));
    let jwks = JwkSet {
        keys: vec![future, serde_json::from_value(rsa["jwk"].clone()).unwrap()],
    };
    let header = Header {
        alg: Algorithm::RS256,
        kid: Some("unknown-key".into()),
        ..Header::default()
    };
    assert_eq!(
        select_jwk(&header, &jwks, Algorithm::RS256),
        Err(JwkSelectError::NoSuitableKey)
    );
}
