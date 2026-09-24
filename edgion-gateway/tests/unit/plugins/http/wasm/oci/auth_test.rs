use super::*;

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[test]
fn selects_by_exact_host_and_legacy_url_key() {
    let json = format!(
        r#"{{"auths":{{"registry.internal:5000":{{"auth":"{}"}},"https://index.docker.io/v1/":{{"auth":"{}"}}}}}}"#,
        b64("alice:s3cret"),
        b64("bob:hub")
    );
    let a = select_registry_auth(&json, "registry.internal:5000").unwrap().unwrap();
    assert_eq!((a.username.as_str(), a.password.as_str()), ("alice", "s3cret"));
    // docker.io API host matches the legacy key.
    let b = select_registry_auth(&json, "registry-1.docker.io").unwrap().unwrap();
    assert_eq!(b.username, "bob");
    // Unknown registry: anonymous, not an error.
    assert!(select_registry_auth(&json, "other.example.com").unwrap().is_none());
}

#[test]
fn credential_selection_has_explicit_exact_and_docker_hub_precedence() {
    let cases = [
        r#"{"auths":{"https://r.io/v1/":{"username":"url","password":"url-pass"},"r.io":{"username":"exact","password":"exact-pass"}}}"#,
        r#"{"auths":{"r.io":{"username":"exact","password":"exact-pass"},"https://r.io/v1/":{"username":"url","password":"url-pass"}}}"#,
    ];
    for json in cases {
        let selected = select_registry_auth(json, "r.io").unwrap().unwrap();
        assert_eq!(
            (selected.username.as_str(), selected.password.as_str()),
            ("exact", "exact-pass")
        );
    }

    let docker = r#"{"auths":{"index.docker.io":{"username":"index","password":"index-pass"},"docker.io":{"username":"docker","password":"docker-pass"},"registry-1.docker.io":{"username":"api","password":"api-pass"}}}"#;
    let selected = select_registry_auth(docker, "registry-1.docker.io").unwrap().unwrap();
    assert_eq!(
        (selected.username.as_str(), selected.password.as_str()),
        ("api", "api-pass")
    );

    let aliases = r#"{"auths":{"index.docker.io":{"username":"index","password":"index-pass"},"docker.io":{"username":"docker","password":"docker-pass"}}}"#;
    let selected = select_registry_auth(aliases, "registry-1.docker.io").unwrap().unwrap();
    assert_eq!(
        (selected.username.as_str(), selected.password.as_str()),
        ("docker", "docker-pass")
    );

    let decorated_cases = [
        r#"{"auths":{"https://r.io/a":{"username":"https","password":"https-pass"},"http://r.io/z":{"username":"http","password":"http-pass"}}}"#,
        r#"{"auths":{"http://r.io/z":{"username":"http","password":"http-pass"},"https://r.io/a":{"username":"https","password":"https-pass"}}}"#,
    ];
    for json in decorated_cases {
        let selected = select_registry_auth(json, "r.io").unwrap().unwrap();
        assert_eq!(
            (selected.username.as_str(), selected.password.as_str()),
            ("http", "http-pass")
        );
    }
}

#[test]
fn malformed_higher_priority_credentials_fail_closed() {
    let json = r#"{"auths":{"r.io":{"auth":"not-base64!"},"https://r.io/v1/":{"username":"fallback","password":"fallback-pass"}}}"#;
    assert!(select_registry_auth(json, "r.io").is_err());
}

#[test]
fn username_password_fields_work_and_malformed_errors() {
    let json = r#"{"auths":{"r.io":{"username":"u","password":"p"}}}"#;
    let a = select_registry_auth(json, "r.io").unwrap().unwrap();
    assert_eq!((a.username.as_str(), a.password.as_str()), ("u", "p"));

    let auth_wins = format!(
        r#"{{"auths":{{"r.io":{{"auth":"{}","username":"fallback","password":"fallback-pass"}}}}}}"#,
        b64("encoded:encoded-pass")
    );
    let selected = select_registry_auth(&auth_wins, "r.io").unwrap().unwrap();
    assert_eq!(
        (selected.username.as_str(), selected.password.as_str()),
        ("encoded", "encoded-pass")
    );

    assert!(select_registry_auth("not-json", "r.io").is_err());
    let no_colon = format!(r#"{{"auths":{{"r.io":{{"auth":"{}"}}}}}}"#, b64("nocolon"));
    assert!(select_registry_auth(&no_colon, "r.io").is_err());
}

#[test]
fn parses_bearer_challenge() {
    let c = parse_bearer_challenge(
        r#"Bearer realm="https://auth.example.com/token",service="registry.example.com",scope="repository:a/b:pull""#,
    )
    .unwrap();
    assert_eq!(c.realm, "https://auth.example.com/token");
    assert_eq!(c.service.as_deref(), Some("registry.example.com"));
    assert_eq!(c.scope.as_deref(), Some("repository:a/b:pull"));
    assert!(parse_bearer_challenge("Basic realm=\"x\"").is_none());
    assert!(parse_bearer_challenge("Bearer service=\"no-realm\"").is_none());
}

#[test]
fn bearer_parser_handles_case_tokens_quoted_commas_and_pairs() {
    let challenge = parse_bearer_challenge(
            "  bEaReR\tReAlM = \"https://auth.example/token?a=one,two\\\"three\\\\four\", SERVICE=registry.example, Scope=\"repository:a/b:pull\", extension=\"x,y\"  ",
        )
        .unwrap();
    assert_eq!(challenge.realm, "https://auth.example/token?a=one,two\"three\\four");
    assert_eq!(challenge.service.as_deref(), Some("registry.example"));
    assert_eq!(challenge.scope.as_deref(), Some("repository:a/b:pull"));
}

#[test]
fn bearer_parser_rejects_malformed_ambiguous_and_oversized_inputs() {
    for header in [
        r#"Bearer realm="unterminated"#,
        r#"Bearer realm="dangling\""#,
        r#"Bearer realm="https://auth", realm="https://other""#,
        r#"Bearer realm="https://auth", SERVICE=one, service=two"#,
        r#"Bearer realm="https://auth" trailing=value"#,
        r#"Bearer realm="https://auth","#,
        r#"Bearer realm="https://auth", bad"#,
    ] {
        assert!(
            parse_bearer_challenge(header).is_none(),
            "accepted malformed header: {header}"
        );
    }

    let too_many = format!(
        "Bearer realm=\"https://auth.example\",{}",
        (0..MAX_BEARER_CHALLENGE_PARAMS)
            .map(|index| format!("x{index}=value"))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(parse_bearer_challenge(&too_many).is_none());
    assert!(parse_bearer_challenge(&format!("Bearer realm=\"{}\"", "x".repeat(MAX_BEARER_CHALLENGE_BYTES))).is_none());
}

#[test]
fn debug_output_redacts_password() {
    let auth = RegistryAuth {
        username: "alice".to_string(),
        password: "s3cret".to_string(),
    };
    let dbg = format!("{auth:?}");
    assert!(!dbg.contains("s3cret"), "password must not appear in Debug: {dbg}");
    assert!(dbg.contains("[redacted]"));
}
