//! Unit tests for `edgion_tls.rs` (see `super`).
use super::*;

fn create_tls(hosts: Vec<&str>) -> EdgionTls {
    EdgionTls {
        metadata: Default::default(),
        spec: EdgionTlsSpec {
            parent_refs: None,
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "test-secret".to_string(),
                namespace: Some("default".to_string()),
            },
            client_auth: None,
            min_tls_version: None,
            ciphers: None,
            secret: None,
            resolved_listeners: None,
            resolved_log_labels: None,
        },
        status: None,
    }
}

#[test]
fn test_yaml_deserialization_with_camel_case() {
    // Test that YAML with camelCase field names can be correctly deserialized
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parentRefs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
    - "*.example.com"
  secretRef:
    name: test-secret
    namespace: default
"#;

    let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
    assert!(tls.is_ok(), "Failed to deserialize YAML: {:?}", tls.err());

    let tls = tls.unwrap();
    assert_eq!(tls.metadata.name, Some("test-tls".to_string()));
    assert_eq!(tls.metadata.namespace, Some("default".to_string()));

    // Verify parentRefs
    assert!(tls.spec.parent_refs.is_some());
    let parent_refs = tls.spec.parent_refs.as_ref().unwrap();
    assert_eq!(parent_refs.len(), 1);
    assert_eq!(parent_refs[0].name, "example-gateway");
    assert_eq!(parent_refs[0].namespace, Some("default".to_string()));

    // Verify hosts
    assert_eq!(tls.spec.hosts.len(), 2);
    assert_eq!(tls.spec.hosts[0], "example.com");
    assert_eq!(tls.spec.hosts[1], "*.example.com");

    // Verify secretRef
    assert_eq!(tls.spec.secret_ref.name, "test-secret");
    assert_eq!(tls.spec.secret_ref.namespace, Some("default".to_string()));
}

#[test]
fn test_yaml_serialization_produces_camel_case() {
    // Test that serialization produces camelCase field names
    let tls = EdgionTls {
        metadata: kube::api::ObjectMeta {
            name: Some("test-tls".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: EdgionTlsSpec {
            parent_refs: Some(vec![ParentReference {
                group: None,
                kind: None,
                name: "example-gateway".to_string(),
                namespace: Some("default".to_string()),
                section_name: None,
                port: None,
            }]),
            hosts: vec!["example.com".to_string(), "*.example.com".to_string()],
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "test-secret".to_string(),
                namespace: Some("default".to_string()),
            },
            client_auth: None,
            min_tls_version: None,
            ciphers: None,
            secret: None,
            resolved_listeners: None,
            resolved_log_labels: None,
        },
        status: None,
    };

    let yaml = serde_yaml::to_string(&tls).expect("Failed to serialize to YAML");

    // Verify camelCase field names in serialized YAML
    assert!(
        yaml.contains("parentRefs:"),
        "Serialized YAML should contain 'parentRefs'"
    );
    assert!(
        yaml.contains("secretRef:"),
        "Serialized YAML should contain 'secretRef'"
    );

    // Verify snake_case is NOT present
    assert!(
        !yaml.contains("parent_refs:"),
        "Serialized YAML should NOT contain 'parent_refs'"
    );
    assert!(
        !yaml.contains("secret_ref:"),
        "Serialized YAML should NOT contain 'secret_ref'"
    );
}

#[test]
fn test_yaml_round_trip() {
    // Test that we can deserialize and then serialize without losing data
    let original_yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parentRefs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
    - test.com
  secretRef:
    name: test-secret
    namespace: default
"#;

    let tls: EdgionTls = serde_yaml::from_str(original_yaml).expect("Failed to deserialize original YAML");

    let serialized = serde_yaml::to_string(&tls).expect("Failed to serialize back to YAML");

    let tls2: EdgionTls = serde_yaml::from_str(&serialized).expect("Failed to deserialize serialized YAML");

    // Verify data integrity
    assert_eq!(tls.metadata.name, tls2.metadata.name);
    assert_eq!(tls.metadata.namespace, tls2.metadata.namespace);
    assert_eq!(tls.spec.hosts, tls2.spec.hosts);
    assert_eq!(tls.spec.secret_ref.name, tls2.spec.secret_ref.name);

    if let (Some(pr1), Some(pr2)) = (&tls.spec.parent_refs, &tls2.spec.parent_refs) {
        assert_eq!(pr1.len(), pr2.len());
        assert_eq!(pr1[0].name, pr2[0].name);
    }
}

#[test]
fn test_snake_case_fields_should_fail() {
    // Test that YAML with snake_case field names (wrong naming) will fail or lose data
    // This ensures we catch naming convention errors early
    let yaml_with_snake_case = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parent_refs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
  secret_ref:
    name: test-secret
    namespace: default
"#;

    let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml_with_snake_case);

    // Should succeed in parsing (because parent_refs and secret_ref have defaults)
    // but the fields should be None/default because they don't match_engine the expected camelCase names
    if let Ok(tls) = tls {
        // With camelCase enforcement, snake_case fields are ignored
        // parent_refs should be None (not parsed from parent_refs)
        assert!(
            tls.spec.parent_refs.is_none(),
            "parent_refs (snake_case) should be ignored, expected None"
        );

        // secret_ref is required, so this test would actually fail at deserialization
        // But if it doesn't fail, the secretRef field would have a default/empty value
    } else {
        // Expected: deserialization should fail because secretRef is required
        // and secret_ref (snake_case) is not recognized
        println!("Expected failure: {:?}", tls.err());
    }
}

#[test]
fn test_kubernetes_api_conventions_compliance() {
    // This test documents that our API follows Kubernetes conventions:
    // 1. All field names should be camelCase in YAML
    // 2. First character should be lowercase
    // 3. Acronyms should follow camelCase (e.g., secretRef not secretREF)

    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: convention-test
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hosts:
    - "*.example.com"
  secretRef:
    name: my-secret
"#;

    let result: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
    assert!(
        result.is_ok(),
        "Kubernetes API convention compliant YAML should deserialize successfully"
    );

    let tls = result.unwrap();
    let serialized = serde_yaml::to_string(&tls).unwrap();

    // Verify all conventions are maintained in serialized output
    assert!(serialized.contains("parentRefs:"), "Should use parentRefs (camelCase)");
    assert!(serialized.contains("secretRef:"), "Should use secretRef (camelCase)");
    assert!(
        !serialized.contains("ParentRefs:"),
        "Should NOT use ParentRefs (PascalCase)"
    );
    assert!(
        !serialized.contains("SecretRef:"),
        "Should NOT use SecretRef (PascalCase)"
    );
}

#[test]
fn test_exact_match() {
    let tls = create_tls(vec!["example.com", "test.com"]);

    assert!(tls.matches_hostname("example.com"));
    assert!(tls.matches_hostname("test.com"));
    assert!(!tls.matches_hostname("other.com"));
}

#[test]
fn test_single_wildcard_one_level() {
    let tls = create_tls(vec!["*.aaa.com"]);

    // Should match_engine one level
    assert!(tls.matches_hostname("test.aaa.com"));
    assert!(tls.matches_hostname("foo.aaa.com"));
    assert!(tls.matches_hostname("bar.aaa.com"));

    // Should NOT match_engine multiple levels
    assert!(!tls.matches_hostname("my.test.aaa.com"));
    assert!(!tls.matches_hostname("a.b.aaa.com"));

    // Should NOT match_engine base domain
    assert!(!tls.matches_hostname("aaa.com"));

    // Should NOT match_engine different domain
    assert!(!tls.matches_hostname("test.bbb.com"));
}

#[test]
fn test_double_wildcard() {
    let tls = create_tls(vec!["*.*.aaa.com"]);

    // Should match_engine two levels
    assert!(tls.matches_hostname("my.test.aaa.com"));
    assert!(tls.matches_hostname("a.b.aaa.com"));

    // Should NOT match_engine one level
    assert!(!tls.matches_hostname("test.aaa.com"));

    // Should NOT match_engine three levels
    assert!(!tls.matches_hostname("x.y.z.aaa.com"));
}

#[test]
fn test_invalid_wildcard_with_prefix() {
    // *-api.example.com is INVALID (wildcard not followed by dot)
    let tls = create_tls(vec!["*-api.example.com"]);

    assert!(!tls.matches_hostname("foo-api.example.com"));
    assert!(!tls.matches_hostname("bar-api.example.com"));
}

#[test]
fn test_invalid_wildcard_with_suffix() {
    // api-*.example.com is INVALID (wildcard not at beginning)
    let tls = create_tls(vec!["api-*.example.com"]);

    assert!(!tls.matches_hostname("api-v1.example.com"));
    assert!(!tls.matches_hostname("api-v2.example.com"));
}

#[test]
fn test_case_insensitive() {
    let tls = create_tls(vec!["*.Example.COM"]);

    assert!(tls.matches_hostname("test.example.com"));
    assert!(tls.matches_hostname("TEST.EXAMPLE.COM"));
    assert!(tls.matches_hostname("Test.Example.Com"));
}

#[test]
fn test_multiple_hosts() {
    let tls = create_tls(vec!["*.aaa.com", "*.bbb.com", "exact.ccc.com"]);

    assert!(tls.matches_hostname("test.aaa.com"));
    assert!(tls.matches_hostname("test.bbb.com"));
    assert!(tls.matches_hostname("exact.ccc.com"));

    assert!(!tls.matches_hostname("test.ccc.com"));
    assert!(!tls.matches_hostname("my.test.aaa.com"));
}

#[test]
fn test_invalid_wildcard_in_middle() {
    // foo.*.example.com is INVALID (wildcard not at beginning)
    let tls = create_tls(vec!["foo.*.example.com"]);

    assert!(!tls.matches_hostname("foo.bar.example.com"));
    assert!(!tls.matches_hostname("foo.test.example.com"));
}

#[test]
fn test_invalid_wildcard_mixed() {
    // *.aaa.*.com is INVALID (wildcard in the middle)
    let tls = create_tls(vec!["*.aaa.*.com"]);

    assert!(!tls.matches_hostname("test.aaa.example.com"));
    assert!(!tls.matches_hostname("foo.aaa.bar.com"));
}

#[test]
fn test_triple_wildcard() {
    let tls = create_tls(vec!["*.*.*.example.com"]);

    // Should match_engine three levels
    assert!(tls.matches_hostname("a.b.c.example.com"));
    assert!(tls.matches_hostname("foo.bar.baz.example.com"));

    // Should NOT match_engine two levels
    assert!(!tls.matches_hostname("a.b.example.com"));

    // Should NOT match_engine four levels
    assert!(!tls.matches_hostname("a.b.c.d.example.com"));
}

#[test]
fn test_empty_hostname() {
    let tls = create_tls(vec!["*.example.com"]);

    assert!(!tls.matches_hostname(""));
}

#[test]
fn test_no_hosts() {
    let tls = create_tls(vec![]);

    assert!(!tls.matches_hostname("test.example.com"));
}

#[test]
fn test_hostname_longer_than_pattern() {
    let tls = create_tls(vec!["*.example.com"]);

    // Hostname has extra characters at the end (not a valid domain)
    assert!(!tls.matches_hostname("aaa.example.coma"));

    // Hostname has extra domain level at the end
    assert!(!tls.matches_hostname("aaa.example.com.us"));

    // Hostname has extra domain level at the beginning
    assert!(!tls.matches_hostname("sub.aaa.example.com"));

    // Valid match_engine - exactly one level before example.com
    assert!(tls.matches_hostname("aaa.example.com"));
}

#[test]
fn test_hostname_longer_with_double_wildcard() {
    let tls = create_tls(vec!["*.*.example.com"]);

    // Hostname has extra characters at the end
    assert!(!tls.matches_hostname("a.b.example.coma"));

    // Hostname has extra domain level at the end
    assert!(!tls.matches_hostname("a.b.example.com.us"));

    // Hostname has extra domain level at the beginning
    assert!(!tls.matches_hostname("c.a.b.example.com"));

    // Valid match_engine - exactly two levels before example.com
    assert!(tls.matches_hostname("a.b.example.com"));
}

#[test]
fn test_client_auth_deserialization() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-test
  namespace: default
spec:
  hosts:
    - api.example.com
  secretRef:
    name: server-tls
    namespace: default
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: default
    verifyDepth: 2
    allowedSans:
      - "client1.example.com"
      - "*.internal.example.com"
    allowedCns:
      - "AdminClient"
"#;

    let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
    assert!(tls.is_ok(), "Failed to deserialize mTLS YAML: {:?}", tls.err());

    let tls = tls.unwrap();
    let client_auth = tls.spec.client_auth.as_ref().unwrap();

    assert_eq!(client_auth.mode, ClientAuthMode::Mutual);
    assert_eq!(client_auth.ca_secret_ref.as_ref().unwrap().name, "client-ca");
    assert_eq!(client_auth.verify_depth, 2);
    // TODO: Re-enable when SAN/CN whitelist is implemented
    // assert_eq!(client_auth.allowed_sans.as_ref().unwrap().len(), 2);
    // assert_eq!(client_auth.allowed_cns.as_ref().unwrap().len(), 1);
}

#[test]
fn test_client_auth_mode_default() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    caSecretRef:
      name: client-ca
"#;

    let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
    let client_auth = tls.spec.client_auth.as_ref().unwrap();

    // Default mode should be Terminate
    assert_eq!(client_auth.mode, ClientAuthMode::Terminate);
}

#[test]
fn test_client_auth_verify_depth_default() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
"#;

    let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
    let client_auth = tls.spec.client_auth.as_ref().unwrap();

    // Default verify_depth should be 1
    assert_eq!(client_auth.verify_depth, 1);

    // Test serialization - default verify_depth should be omitted
    let serialized = serde_yaml::to_string(&tls).unwrap();
    assert!(
        !serialized.contains("verifyDepth"),
        "Default verifyDepth should be omitted in serialization"
    );
}

#[test]
fn test_client_auth_optional_fields() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
"#;

    let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
    let client_auth = tls.spec.client_auth.as_ref().unwrap();

    assert_eq!(client_auth.mode, ClientAuthMode::OptionalMutual);
    // TODO: Re-enable when SAN/CN whitelist is implemented
    // assert!(client_auth.allowed_sans.is_none());
    // assert!(client_auth.allowed_cns.is_none());
}

#[test]
fn test_client_auth_helper_methods() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
"#;

    let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(tls.client_auth_mode(), ClientAuthMode::Mutual);
    assert!(tls.is_mtls_enabled());
}

#[test]
fn test_no_client_auth() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
"#;

    let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();

    assert!(tls.spec.client_auth.is_none());
    assert_eq!(tls.client_auth_mode(), ClientAuthMode::Terminate);
    assert!(!tls.is_mtls_enabled());
}

#[test]
fn test_pre_parse_populates_resolved_log_labels() {
    use crate::constants::annotations::edgion::LOG_LABEL_PREFIX;

    let mut tls = create_tls(vec!["example.com"]);
    // Operator log-label annotation/label sources resolved at pre_parse time so
    // the TLS handshake callback (ssl.log) reads a cached Arc, not metadata.
    tls.metadata.annotations = Some(
        [(format!("{LOG_LABEL_PREFIX}tenant"), "internal".to_string())]
            .into_iter()
            .collect(),
    );
    tls.metadata.labels = Some(
        [(format!("{LOG_LABEL_PREFIX}env"), "prod".to_string())]
            .into_iter()
            .collect(),
    );

    assert!(tls.spec.resolved_log_labels.is_none(), "cache empty before pre_parse");

    // Trait method must be wired in `resource/meta/impls.rs`; this guards against
    // a future edit dropping the EdgionTls pre_parse override (labels would then
    // silently vanish from ssl.log with no error).
    <EdgionTls as crate::ResourceMeta>::pre_parse(&mut tls);

    let resolved = tls.spec.resolved_log_labels.expect("pre_parse populates labels");
    assert_eq!(resolved.get("tenant").map(String::as_str), Some("internal"));
    assert_eq!(resolved.get("env").map(String::as_str), Some("prod"));
    assert_eq!(resolved.len(), 2);
}

#[test]
fn test_resolved_log_labels_skipped_in_serialization() {
    // The cache is a pure in-process field: it must never leak into the
    // conf_sync payload or the CRD (serde(skip) + schemars(skip)).
    let mut tls = create_tls(vec!["example.com"]);
    tls.spec.resolved_log_labels = Some(std::sync::Arc::new(
        [("env".to_string(), "prod".to_string())].into_iter().collect(),
    ));
    let json = serde_json::to_string(&tls.spec).expect("serialize spec");
    assert!(
        !json.contains("resolvedLogLabels") && !json.contains("resolved_log_labels"),
        "resolved_log_labels must be serde(skip): {json}"
    );
}
