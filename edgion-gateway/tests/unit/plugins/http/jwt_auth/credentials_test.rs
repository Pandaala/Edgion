use super::*;

fn group(algorithm: JwtAlgorithm, credential: ResolvedJwtCredential) -> ResolvedJwtIssuerGroup {
    ResolvedJwtIssuerGroup {
        group: "test-group".to_string(),
        algorithm,
        audiences: None,
        credentials: HashMap::from([("test-kid".to_string(), credential)]),
    }
}

#[test]
fn store_prebuilds_invalid_rsa_key_as_cached_error() {
    let credential = ResolvedJwtCredential {
        key: Some("test-kid".to_string()),
        secret: None,
        public_key: Some("not a PEM key".to_string()),
    };
    let store = JwtIssuerGroupStore::new(
        HashMap::from([(
            "https://test.example".to_string(),
            group(JwtAlgorithm::RS256, credential),
        )]),
        false,
        0,
    );

    let cached_group = store
        .lookup_by_iss("https://test.example")
        .expect("issuer must be cached");
    assert!(matches!(
        cached_group.lookup_decoding_key("test-kid"),
        Some(Err("invalid RSA PEM"))
    ));
    assert!(matches!(
        cached_group.lookup_decoding_key("test-kid"),
        Some(Err("invalid RSA PEM"))
    ));
}

#[test]
fn prepared_validation_preserves_exp_and_configures_optional_claims() {
    let configured = build_validation(
        JwtAlgorithm::HS256,
        Some(vec!["api-a".to_string(), "api-b".to_string()]),
        17,
    );
    assert!(configured.required_spec_claims.contains("exp"));
    assert!(configured.required_spec_claims.contains("aud"));
    assert!(configured.validate_aud);
    assert!(configured.validate_nbf);
    assert_eq!(configured.leeway, 17);
    assert_eq!(configured.aud.as_ref().map(|aud| aud.len()), Some(2));

    for audiences in [None, Some(Vec::new())] {
        let disabled = build_validation(JwtAlgorithm::HS256, audiences, 0);
        assert!(disabled.required_spec_claims.contains("exp"));
        assert!(!disabled.required_spec_claims.contains("aud"));
        assert!(!disabled.validate_aud);
        assert!(disabled.validate_nbf);
    }
}

#[test]
fn prepared_key_and_validation_are_reused() {
    let credential = ResolvedJwtCredential {
        key: Some("test-kid".to_string()),
        secret: Some("a-secret-long-enough-for-the-test".to_string()),
        public_key: None,
    };
    let store = JwtIssuerGroupStore::new(
        HashMap::from([(
            "https://test.example".to_string(),
            group(JwtAlgorithm::HS256, credential),
        )]),
        false,
        9,
    );
    let first_group = store.lookup_by_iss("https://test.example").unwrap();
    let second_group = store.lookup_by_iss("https://test.example").unwrap();
    assert!(std::ptr::eq(first_group, second_group));
    assert!(std::ptr::eq(first_group.validation(), second_group.validation()));
    assert!(std::ptr::eq(
        first_group.lookup_decoding_key("test-kid").unwrap().unwrap(),
        second_group.lookup_decoding_key("test-kid").unwrap().unwrap(),
    ));
}
