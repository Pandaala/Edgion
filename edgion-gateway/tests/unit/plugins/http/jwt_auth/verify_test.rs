use super::*;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, JwtAuthConfig, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use std::collections::HashMap;

fn make_hs_group(name: &str, kid: &str, secret_bytes: &[u8]) -> ResolvedJwtIssuerGroup {
    let mut creds = HashMap::new();
    creds.insert(
        kid.to_string(),
        ResolvedJwtCredential {
            key: Some(kid.to_string()),
            secret: Some(std::str::from_utf8(secret_bytes).unwrap().to_string()),
            public_key: None,
        },
    );
    ResolvedJwtIssuerGroup {
        group: name.to_string(),
        algorithm: JwtAlgorithm::HS256,
        audiences: None,
        credentials: creds,
    }
}

fn config_default_keyclaim() -> JwtAuthConfig {
    JwtAuthConfig::default()
}

fn make_token(alg: Algorithm, kid: &str, iss: &str, secret_bytes: &[u8]) -> String {
    let claims = json!({
        "iss": iss,
        "key": kid,
        "sub": "alice",
        "exp": (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600),
    });
    let header = Header::new(alg);
    encode(&header, &claims, &EncodingKey::from_secret(secret_bytes)).unwrap()
}

#[test]
fn routing_decode_preserves_malformed_and_security_failure_precedence() {
    use base64::Engine;
    let encode_part = |raw: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let secret = b"hs256-secret-at-least-32-bytes-long-padding";
    let store = JwtIssuerGroupStore::new(
        HashMap::from([(
            "https://a.example".to_string(),
            make_hs_group("partner-a", "k1", secret),
        )]),
        false,
        0,
    );
    let config = config_default_keyclaim();
    let valid_header = encode_part(r#"{"alg":"HS256"}"#);
    let payload = encode_part(r#"{"iss":"https://a.example","key":"k1","exp":4102444800}"#);
    let mut cases = vec![
        (String::new(), VerifyDeny::Malformed, "Deny malformed token"),
        (valid_header.clone(), VerifyDeny::Malformed, "Deny malformed token"),
        (
            format!("{valid_header}.{payload}"),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ),
        (
            format!("extra.{valid_header}.{payload}.signature"),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ),
        (
            format!("!.{payload}.signature"),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ),
        (
            format!("{valid_header}.!.signature"),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ),
        (
            format!("{valid_header}.{payload}."),
            VerifyDeny::SignatureFail,
            "Deny invalid signature",
        ),
    ];
    for raw in [
        "{",
        "[]",
        "{}",
        r#"{"alg":"none"}"#,
        r#"{"alg":1}"#,
        r#"{"alg":"HS256","alg":"HS384"}"#,
    ] {
        cases.push((
            format!("{}.{payload}.signature", encode_part(raw)),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ));
    }
    for raw in ["{", "[]", "null"] {
        cases.push((
            format!("{valid_header}.{}.signature", encode_part(raw)),
            VerifyDeny::Malformed,
            "Deny malformed token",
        ));
    }
    for (raw, expected, tag) in [
        (r#"{"key":"k1"}"#, VerifyDeny::MissingIss, "Deny missing iss"),
        (
            r#"{"iss":"https://unknown.example","key":"missing"}"#,
            VerifyDeny::UnknownIss,
            "Deny unknown iss",
        ),
        (
            r#"{"iss":"https://a.example","key":"missing"}"#,
            VerifyDeny::BadAlgorithm,
            "Deny bad algorithm",
        ),
    ] {
        cases.push((
            format!("{}.{}.signature", encode_part(r#"{"alg":"HS384"}"#), encode_part(raw)),
            expected,
            tag,
        ));
    }
    for (token, expected, tag) in cases {
        // Freeze the old prefix as a dependency-upgrade equivalence oracle.
        let old_prefix = jsonwebtoken::decode_header(&token).and_then(|header| {
            decode::<Value>(&token, store.peek_decoding_key(), store.peek_validation())
                .map(|peek| (header.alg, peek.claims))
        });
        let new_prefix = decode::<Value>(&token, store.peek_decoding_key(), store.peek_validation())
            .map(|peek| (peek.header.alg, peek.claims));
        assert_eq!(old_prefix.is_ok(), new_prefix.is_ok());
        if let (Ok(old), Ok(new)) = (old_prefix, new_prefix) {
            assert_eq!(old, new);
        }
        let mut log = PluginLog::new("JwtAuth");
        let actual = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert_eq!(std::mem::discriminant(&actual), std::mem::discriminant(&expected));
        assert!(log.contains(tag));
        let rendered = log.render_for_test();
        for secret_value in [
            "partner-a",
            "https://a.example",
            "https://unknown.example",
            "k1",
            "HS384",
        ] {
            assert!(!rendered.contains(secret_value));
        }
    }
}

#[test]
fn verify_hits_correct_group_by_iss() {
    let secret = b"hs256-secret-at-least-32-bytes-long-padding";
    let group_a = make_hs_group("partner-a", "k1", secret);
    let mut by_iss = HashMap::new();
    by_iss.insert("https://a.example".to_string(), group_a);
    let store = JwtIssuerGroupStore::new(by_iss, false, 0);
    let config = config_default_keyclaim();
    let mut log = PluginLog::new("JwtAuth");

    let token = make_token(Algorithm::HS256, "k1", "https://a.example", secret);
    let ok = verify_jwt(&token, &store, &config, &mut log).unwrap();
    drop(token);
    assert_eq!(ok.claims()["iss"], "https://a.example");
    assert_eq!(ok.issuer(), "https://a.example");
    assert!(std::ptr::eq(ok.issuer(), ok.claims()["iss"].as_str().unwrap()));
    assert_eq!(ok.group.group, "partner-a");
}

#[test]
fn verify_rejects_unknown_iss() {
    let secret = b"hs256-secret-at-least-32-bytes-long-padding";
    let group_a = make_hs_group("partner-a", "k1", secret);
    let mut by_iss = HashMap::new();
    by_iss.insert("https://a.example".to_string(), group_a);
    let store = JwtIssuerGroupStore::new(by_iss, false, 0);
    let config = config_default_keyclaim();
    let mut log = PluginLog::new("JwtAuth");

    let token = make_token(Algorithm::HS256, "k1", "https://unknown.example", secret);
    let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
    assert!(matches!(err, VerifyDeny::UnknownIss));
}

#[test]
fn verify_rejects_iss_trailing_slash_mismatch() {
    // Token iss has trailing slash; group iss does not. iss matching is exact, no normalization.
    let secret = b"hs256-secret-at-least-32-bytes-long-padding";
    let group_a = make_hs_group("partner-a", "k1", secret);
    let mut by_iss = HashMap::new();
    by_iss.insert("https://a.example".to_string(), group_a); // no slash
    let store = JwtIssuerGroupStore::new(by_iss, false, 0);
    let config = config_default_keyclaim();
    let mut log = PluginLog::new("JwtAuth");

    let token = make_token(Algorithm::HS256, "k1", "https://a.example/", secret); // with slash
    let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
    assert!(
        matches!(err, VerifyDeny::UnknownIss),
        "got: {:?}",
        std::mem::discriminant(&err)
    );
}

#[test]
fn verify_rejects_algorithm_confusion_attack() {
    // partner-a is HS256. Token claims iss=partner-a but signed with HS384.
    // header.alg=HS384 vs group.algorithm=HS256 -> Deny bad algorithm.
    let secret = b"hs256-secret-at-least-32-bytes-long-padding";
    let group_a = make_hs_group("partner-a", "k1", secret);
    let mut by_iss = HashMap::new();
    by_iss.insert("https://a.example".to_string(), group_a);
    let store = JwtIssuerGroupStore::new(by_iss, false, 0);
    let config = config_default_keyclaim();
    let mut log = PluginLog::new("JwtAuth");

    let token = make_token(Algorithm::HS384, "k1", "https://a.example", secret);
    let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
    assert!(matches!(err, VerifyDeny::BadAlgorithm));
}

#[test]
fn verify_rejects_kid_from_other_group() {
    // partner-a has kid=ka; partner-b has kid=kb.
    // Token claims iss=partner-b but kid=ka (from partner-a).
    // Routing -> partner-b group -> ka not in partner-b.credentials -> Deny missing key.
    let secret_a = b"hs256-secret-a-padding-padding-padding-padding";
    let secret_b = b"hs256-secret-b-padding-padding-padding-padding";
    let group_a = make_hs_group("partner-a", "ka", secret_a);
    let group_b = make_hs_group("partner-b", "kb", secret_b);
    let mut by_iss = HashMap::new();
    by_iss.insert("https://a.example".to_string(), group_a);
    by_iss.insert("https://b.example".to_string(), group_b);
    let store = JwtIssuerGroupStore::new(by_iss, false, 0);
    let config = config_default_keyclaim();
    let mut log = PluginLog::new("JwtAuth");

    // Sign with secret_a (would verify only if routed to group_a), but
    // claim iss=https://b.example. Routing -> group_b -> kid `ka` not in
    // group_b's credentials -> Deny missing key.
    let token = make_token(Algorithm::HS256, "ka", "https://b.example", secret_a);
    let err = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
    assert!(matches!(err, VerifyDeny::MissingKey));
}

#[test]
fn verify_reuses_cached_malformed_key_error_without_logging_key_context() {
    let group = ResolvedJwtIssuerGroup {
        group: "sensitive-group".to_string(),
        algorithm: JwtAlgorithm::HS256,
        audiences: None,
        credentials: HashMap::from([(
            "sensitive-kid".to_string(),
            ResolvedJwtCredential {
                key: Some("sensitive-kid".to_string()),
                secret: Some("not-valid-base64!".to_string()),
                public_key: None,
            },
        )]),
    };
    let store = JwtIssuerGroupStore::new(
        HashMap::from([("https://sensitive.example".to_string(), group)]),
        true,
        0,
    );
    let config = config_default_keyclaim();
    let token = make_token(
        Algorithm::HS256,
        "sensitive-kid",
        "https://sensitive.example",
        b"unused-signing-secret-at-least-32-bytes",
    );

    for _ in 0..2 {
        let mut log = PluginLog::new("JwtAuth");
        let error = verify_jwt(&token, &store, &config, &mut log).unwrap_err();
        assert!(matches!(error, VerifyDeny::InvalidCredentials));
        assert!(log.contains("Deny invalid credentials"));
        let serialized = serde_json::to_string(&log).unwrap();
        for forbidden in ["sensitive-group", "sensitive-kid", "https://sensitive.example"] {
            assert!(!serialized.contains(forbidden), "failure log leaked `{forbidden}`");
        }
    }
}

#[test]
fn numeric_date_matches_jsonwebtoken_rounding_boundary() {
    assert_eq!(numeric_date(&json!(42_u64)), Some(42));
    assert_eq!(numeric_date(&json!(42.49)), Some(42));
    assert_eq!(numeric_date(&json!(42.5)), Some(43));
    assert_eq!(numeric_date(&json!(42.51)), Some(43));
    assert_eq!(numeric_date(&json!(-0.1)), None);

    let max_as_float = serde_json::Number::from_f64(u64::MAX as f64).unwrap();
    assert_eq!(numeric_date(&Value::Number(max_as_float)), None);
}

#[test]
fn verify_ok_debug_is_fully_opaque() {
    let secret = b"sensitive-key-material-at-least-32-bytes";
    let store = JwtIssuerGroupStore::new(
        HashMap::from([(
            "sensitive-issuer".to_string(),
            make_hs_group("sensitive-group", "sensitive-kid", secret),
        )]),
        false,
        0,
    );
    let ok = VerifyOk {
        group: store.lookup_by_iss("sensitive-issuer").unwrap(),
        claims: json!({
            "iss": "sensitive-issuer",
            "sub": "sensitive-claim",
            "token": "sensitive-token"
        }),
    };

    let debug = format!("{ok:?}");
    assert_eq!(debug, "VerifyOk { .. }");
    for sentinel in [
        "sensitive-issuer",
        "sensitive-group",
        "sensitive-kid",
        "sensitive-key-material",
        "sensitive-claim",
        "sensitive-token",
    ] {
        assert!(!debug.contains(sentinel), "Debug leaked `{sentinel}`: {debug}");
    }
}
