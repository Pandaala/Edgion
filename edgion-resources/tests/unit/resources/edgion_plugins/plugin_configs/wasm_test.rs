use super::*;

fn file_config() -> WasmConfig {
    WasmConfig {
        source: WasmSourceSpec {
            file: Some("./demo.wasm".to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    // No source set at all → invalid.
    let config = WasmConfig::default();
    assert!(config.validation_error().is_some());
}

#[test]
fn valid_file_source_passes() {
    assert!(file_config().validate().is_ok());
}

#[test]
fn two_sources_rejected() {
    let mut c = file_config();
    c.source.inline = Some("AA==".to_string());
    assert!(c.validate().unwrap_err().contains("exactly one"));
}

#[test]
fn url_requires_sha256() {
    let mut c = WasmConfig::default();
    c.source.url = Some("https://example.com/m.wasm".to_string());
    assert!(c.validate().unwrap_err().contains("sha256"));
    c.sha256 = Some("a".repeat(64));
    assert!(c.validate().is_ok());
}

#[test]
fn url_metrics_label_is_redacted_bounded_and_generation_stable() {
    let source = |url: &str| WasmSourceSpec {
        url: Some(url.to_string()),
        ..Default::default()
    };
    let first = source("https://alice:password@artifacts.example/tenant/private/module.wasm?token=first#secret")
        .metrics_label();
    let rotated =
        source("https://bob:other@artifacts.example/tenant/private/module.wasm?token=second#other").metrics_label();

    assert_eq!(first, rotated);
    assert!(first.starts_with("url:https://artifacts.example#"), "got: {first}");
    for secret in [
        "alice",
        "password",
        "tenant",
        "private",
        "module.wasm",
        "token",
        "secret",
    ] {
        assert!(!first.contains(secret), "label leaked {secret}: {first}");
    }
    assert!(first.len() <= MAX_SOURCE_METRICS_LABEL_BYTES);

    let other_path = source("https://artifacts.example/other/module.wasm").metrics_label();
    assert_ne!(first, other_path, "different URL paths need distinct source identities");

    let long_host = format!("https://{}.example/module.wasm", "a".repeat(180));
    let long_label = source(&long_host).metrics_label();
    assert!(long_label.len() <= MAX_SOURCE_METRICS_LABEL_BYTES, "got: {long_label}");
}

#[test]
fn invalid_url_validation_does_not_echo_the_configured_value() {
    let mut c = WasmConfig::default();
    let configured = "file://user:password@host/private/module.wasm?token=secret";
    c.source.url = Some(configured.to_string());
    c.sha256 = Some("a".repeat(64));
    let error = c.validate().unwrap_err();
    assert!(!error.contains(configured), "got: {error}");
    assert!(!error.contains("password"), "got: {error}");
}

#[test]
fn bad_sha256_rejected() {
    let mut c = file_config();
    c.sha256 = Some("xyz".to_string());
    assert!(c.validate().is_err());
}

#[test]
fn bad_inline_base64_rejected() {
    let mut c = WasmConfig::default();
    c.source.inline = Some("!!not-base64!!".to_string());
    assert!(c.validate().unwrap_err().contains("base64"));
}

#[test]
fn fetch_requires_url_source() {
    let mut c = file_config();
    c.source.fetch = Some(WasmFetchSpec::default());
    assert!(c.validate().unwrap_err().contains("source.url"));

    let mut c = WasmConfig::default();
    c.source.url = Some("https://artifacts.internal/m.wasm".to_string());
    c.sha256 = Some("a".repeat(64));
    c.source.fetch = Some(WasmFetchSpec {
        timeout_ms: default_fetch_timeout_ms(),
        ..Default::default()
    });
    assert!(c.validate().is_ok());
}

#[test]
fn fetch_bounds_validated() {
    let mut c = WasmConfig::default();
    c.source.url = Some("https://artifacts.internal/m.wasm".to_string());
    c.sha256 = Some("a".repeat(64));
    let mut fetch = WasmFetchSpec {
        timeout_ms: 0,
        ..Default::default()
    };
    c.source.fetch = Some(fetch.clone());
    assert!(c.validate().unwrap_err().contains("timeoutMs"));

    fetch.timeout_ms = 5000;
    fetch.retry = Some(WasmFetchRetry {
        attempts: 0,
        backoff_ms: 100,
    });
    c.source.fetch = Some(fetch.clone());
    assert!(c.validate().unwrap_err().contains("attempts"));

    fetch.retry = Some(WasmFetchRetry {
        attempts: 3,
        backoff_ms: 60_000,
    });
    c.source.fetch = Some(fetch);
    assert!(c.validate().unwrap_err().contains("backoffMs"));
}

#[test]
fn fetch_serde_camel_case_and_resolved_field_not_deserializable_from_crd_path() {
    let json = r#"{
            "source": {
                "url": "https://artifacts.internal/m.wasm",
                "fetch": {
                    "authHeaderSecretRef": {"name": "repo-token", "key": "authorization"},
                    "timeoutMs": 3000,
                    "retry": {"attempts": 2, "backoffMs": 250}
                }
            },
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }"#;
    let c: WasmConfig = serde_json::from_str(json).unwrap();
    let fetch = c.source.fetch.as_ref().unwrap();
    assert_eq!(fetch.timeout_ms, 3000);
    assert_eq!(fetch.retry.as_ref().unwrap().attempts, 2);
    let auth = fetch.auth_header_secret_ref.as_ref().unwrap();
    assert_eq!(auth.secret.name, "repo-token");
    assert_eq!(auth.key, "authorization");
    assert!(c.validate().is_ok());
}

#[test]
fn callout_origin_normalization() {
    for (raw, expected) in [
        ("https://auth.example.com", Some("https://auth.example.com:443")),
        ("http://svc:8080/", Some("http://svc:8080")),
        ("https://a.example.com/v1", None),
        ("ftp://a.example.com", None),
        ("https://user:pw@a.example.com", None),
        ("https://a.example.com?q=1", None),
    ] {
        assert_eq!(
            normalize_callout_origin(raw).ok().as_deref(),
            expected,
            "diagnostic normalization result for {raw}"
        );
        assert_eq!(
            normalize_callout_origin_quiet(raw).as_deref(),
            expected,
            "quiet runtime normalization result for {raw}"
        );
    }
}

#[test]
fn callout_config_validated() {
    let mut c = file_config();
    c.callout_allowlist = vec!["https://auth.example.com".to_string()];
    assert!(c.validate().is_ok());
    c.callout_allowlist = vec!["https://a.example.com/deny-path".to_string()];
    assert!(c.validate().unwrap_err().contains("calloutAllowlist"));
    c.callout_allowlist = vec![];
    c.callout_timeout_ms = 0;
    assert!(c.validate().unwrap_err().contains("calloutTimeoutMs"));
}

#[test]
fn resolved_auth_header_redacted_under_admin_guard() {
    let fetch = WasmFetchSpec {
        resolved_auth_header: Some("Bearer real-token".to_string()),
        timeout_ms: default_fetch_timeout_ms(),
        ..Default::default()
    };
    // conf_sync path (no guard): real value ships to the gateway.
    let plain = serde_json::to_value(&fetch).unwrap();
    assert_eq!(plain["resolvedAuthHeader"], "Bearer real-token");
    // Admin path (guard active): value collapses to "[redacted]".
    let _guard = crate::redact::AdminRedactGuard::new();
    let redacted = serde_json::to_value(&fetch).unwrap();
    assert_eq!(redacted["resolvedAuthHeader"], "[redacted]");
}

#[test]
fn fetch_does_not_change_cache_key() {
    let mut a = WasmSourceSpec {
        url: Some("https://artifacts.internal/m.wasm".to_string()),
        ..Default::default()
    };
    let key_plain = a.cache_key();
    a.fetch = Some(WasmFetchSpec::default());
    assert_eq!(
        a.cache_key(),
        key_plain,
        "fetch settings must not affect module identity"
    );
}

#[test]
fn oversized_inline_rejected_with_guidance() {
    use base64::Engine as _;
    let mut c = WasmConfig::default();
    let bytes = vec![0u8; MAX_INLINE_DECODED_BYTES + 1];
    c.source.inline = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
    let err = c.validate().unwrap_err();
    assert!(err.contains("inline cap"), "got: {err}");
    assert!(err.contains("file or url"), "got: {err}");
    // At the cap exactly it still passes.
    let bytes = vec![0u8; MAX_INLINE_DECODED_BYTES];
    c.source.inline = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
    assert!(c.validate().is_ok());
}

#[test]
fn timeout_and_pool_bounds() {
    let mut c = file_config();
    c.timeout_ms = 0;
    assert!(c.validate().is_err());
    c.timeout_ms = 10_001;
    assert!(c.validate().is_err());
    c.timeout_ms = 100;
    c.instance_pool_size = 0;
    assert!(c.validate().is_err());
    c.instance_pool_size = 65;
    assert!(c.validate().is_err());
}

#[test]
fn cache_key_distinguishes_equal_length_inline_sources() {
    // Two different inline base64 strings of equal length must not
    // collide in the module cache key (previously keyed by length only).
    let a = WasmSourceSpec {
        inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
        ..Default::default()
    };
    let b = WasmSourceSpec {
        inline: Some("BBBBBBBBBBBBBBBBBBBB".to_string()),
        ..Default::default()
    };
    assert_eq!(a.inline.as_deref().unwrap().len(), b.inline.as_deref().unwrap().len());
    assert_ne!(
        a.cache_key(),
        b.cache_key(),
        "different inline content must not collide"
    );
}

#[test]
fn cache_key_stable_for_identical_inline_source() {
    let a = WasmSourceSpec {
        inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
        ..Default::default()
    };
    let b = WasmSourceSpec {
        inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
        ..Default::default()
    };
    assert_eq!(
        a.cache_key(),
        b.cache_key(),
        "identical inline content must key identically"
    );
}

#[test]
fn serde_camel_case_roundtrip() {
    let json = r#"{
            "source": { "file": "./demo.wasm" },
            "pluginConfig": { "json": {} },
            "vmConfig": { "base64": "/wA=" },
            "failOpen": true,
            "timeoutMs": 50,
            "instancePoolSize": 4
        }"#;
    let c: WasmConfig = serde_json::from_str(json).unwrap();
    assert_eq!(c.source.file.as_deref(), Some("./demo.wasm"));
    assert!(c.fail_open);
    assert_eq!(c.timeout_ms, 50);
    assert_eq!(c.instance_pool_size, 4);
    assert_eq!(c.plugin_config.as_ref().unwrap().guest_bytes().unwrap(), b"{}");
    assert_eq!(c.vm_config.as_ref().unwrap().guest_bytes().unwrap(), [0xff, 0x00]);
    let roundtrip: WasmConfig = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
    assert_eq!(
        roundtrip.plugin_config.unwrap().guest_bytes().unwrap(),
        b"{}",
        "conf_sync serde roundtrip must preserve the representation and effective bytes"
    );
}

#[test]
fn raw_config_accepts_all_three_strict_shapes() {
    let string: WasmRawConfig = serde_json::from_str(r#"{"string":"hello"}"#).unwrap();
    let json: WasmRawConfig = serde_json::from_str(r#"{"json":{"b":2,"a":1}}"#).unwrap();
    let base64: WasmRawConfig = serde_json::from_str(r#"{"base64":"aGVsbG8="}"#).unwrap();
    assert_eq!(string.guest_bytes().unwrap(), b"hello");
    assert_eq!(json.guest_bytes().unwrap(), br#"{"a":1,"b":2}"#);
    assert_eq!(base64.guest_bytes().unwrap(), b"hello");
}

#[test]
fn raw_config_rejects_bare_missing_multiple_and_unknown_shapes() {
    for invalid in [
        r#""legacy""#,
        r#"{}"#,
        r#"{"string":"a","base64":"YQ=="}"#,
        r#"{"string":"a","typo":true}"#,
        r#"{"typo":"a"}"#,
    ] {
        assert!(
            serde_json::from_str::<WasmRawConfig>(invalid).is_err(),
            "shape must be rejected: {invalid}"
        );
    }
}

#[test]
fn raw_config_json_schema_is_a_strict_three_way_union() {
    let schema = serde_json::to_value(schemars::schema_for!(WasmRawConfig)).unwrap();
    let any_of = schema["anyOf"].as_array().expect("untagged union must use anyOf");
    assert_eq!(any_of.len(), 3);
    let rendered = serde_json::to_string(&schema).unwrap();
    for selector in ["string", "json", "base64"] {
        assert!(rendered.contains(&format!("\"{selector}\"")));
    }
    assert_eq!(
        rendered.matches("\"additionalProperties\":false").count(),
        3,
        "every selector object must reject unknown or second fields"
    );
}

#[test]
fn raw_base64_is_strict_canonical_standard() {
    for invalid in ["***", "YQ", "YQ=", "YR==", "YQ==\n"] {
        let config = WasmRawConfig::base64(invalid);
        assert!(config.guest_bytes().is_err(), "base64 must be rejected: {invalid:?}");
    }
    assert_eq!(WasmRawConfig::base64("YQ==").guest_bytes().unwrap(), b"a");
}

#[test]
fn raw_config_decoded_cap_boundary() {
    use base64::Engine as _;

    let at_cap = vec![0xff; MAX_WASM_RAW_CONFIG_BYTES];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&at_cap);
    assert_eq!(encoded.len(), MAX_WASM_RAW_BASE64_BYTES);
    assert_eq!(WasmRawConfig::base64(encoded).guest_bytes().unwrap(), at_cap);

    let over_cap = vec![0xff; MAX_WASM_RAW_CONFIG_BYTES + 1];
    let encoded = base64::engine::general_purpose::STANDARD.encode(over_cap);
    let error = WasmRawConfig::base64(encoded).guest_bytes().unwrap_err();
    assert!(error.contains("64KiB"), "got: {error}");

    assert!(WasmRawConfig::string("x".repeat(MAX_WASM_RAW_CONFIG_BYTES))
        .guest_bytes()
        .is_ok());
    assert!(WasmRawConfig::string("x".repeat(MAX_WASM_RAW_CONFIG_BYTES + 1))
        .guest_bytes()
        .is_err());
}

#[test]
fn raw_json_is_recursive_compact_and_accepts_every_value_kind() {
    let config: WasmRawConfig =
        serde_json::from_str(r#"{"json":{"z":{"b":1,"a":2},"a":[null,true,"x"],"n":1.0}}"#).unwrap();
    assert_eq!(
        config.guest_bytes().unwrap(),
        br#"{"a":[null,true,"x"],"n":1.0,"z":{"a":2,"b":1}}"#
    );

    for (input, expected) in [
        (r#"{"json":null}"#, "null"),
        (r#"{"json":true}"#, "true"),
        (r#"{"json":7}"#, "7"),
        (r#"{"json":"text"}"#, r#""text""#),
        (r#"{"json":[3,2,1]}"#, "[3,2,1]"),
    ] {
        let value: WasmRawConfig = serde_json::from_str(input).unwrap();
        assert_eq!(value.guest_bytes().unwrap(), expected.as_bytes());
    }
}

#[test]
fn raw_json_canonical_bytes_obey_cap() {
    let at_cap_payload = "x".repeat(MAX_WASM_RAW_CONFIG_BYTES - 2);
    let at_cap = WasmRawConfig::Json(WasmRawJson {
        json: Value::String(at_cap_payload),
    });
    assert_eq!(at_cap.guest_bytes().unwrap().len(), MAX_WASM_RAW_CONFIG_BYTES);

    let over_cap = WasmRawConfig::Json(WasmRawJson {
        json: Value::String("x".repeat(MAX_WASM_RAW_CONFIG_BYTES - 1)),
    });
    assert!(over_cap.guest_bytes().is_err());
}

#[test]
fn wasm_validation_names_raw_config_field_without_echoing_content() {
    let marker = "sensitive-marker";
    let mut config = file_config();
    config.plugin_config = Some(WasmRawConfig::string(format!(
        "{marker}{}",
        "x".repeat(MAX_WASM_RAW_CONFIG_BYTES)
    )));
    let error = config.validate().unwrap_err();
    assert!(error.starts_with("pluginConfig decoded value exceeds"));
    assert!(!error.contains(marker));
}

#[test]
fn oci_source_counts_in_exactly_one() {
    // oci alone is a valid single source (sha256 not required).
    let mut c = WasmConfig::default();
    c.source.oci = Some(super::super::wasm_oci::WasmOciSpec {
        image: "registry.internal/wasm/demo:1.0".to_string(),
        ..Default::default()
    });
    assert!(c.validate().is_ok());
    // oci + file violates exactly-one, and the message names oci.
    c.source.file = Some("./demo.wasm".to_string());
    let err = c.validate().unwrap_err();
    assert!(err.contains("file/inline/url/oci"), "got: {err}");
}

#[test]
fn oci_image_syntax_validated() {
    let mut c = WasmConfig::default();
    c.source.oci = Some(super::super::wasm_oci::WasmOciSpec {
        image: "no-registry-host:1.0".to_string(),
        ..Default::default()
    });
    let err = c.validate().unwrap_err();
    assert!(err.contains("source.oci.image"), "got: {err}");
}

#[test]
fn oci_cache_key_and_metrics_label() {
    let mk = |image: &str| WasmSourceSpec {
        oci: Some(super::super::wasm_oci::WasmOciSpec {
            image: image.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = mk("r.io/mod:1.0");
    let b = mk("r.io/mod:2.0");
    assert_ne!(a.cache_key(), b.cache_key());
    assert_eq!(a.metrics_label(), "oci:r.io/mod:1.0");
    let digest = format!("sha256:{}", "c".repeat(64));
    let pinned = mk(&format!("r.io/mod@{digest}"));
    assert_eq!(pinned.metrics_label(), format!("oci:r.io/mod@{}", &"c".repeat(64)[..8]));
    // resolved_pull_secret must not affect module identity.
    let mut c = mk("r.io/mod:1.0");
    c.oci.as_mut().unwrap().resolved_pull_secret = Some("x".to_string());
    assert_eq!(c.cache_key(), a.cache_key());
}

#[test]
fn rejected_callout_origins_never_enter_validation_diagnostics() {
    use crate::resources::edgion_plugins::EdgionPlugins;

    for (raw, expected) in [
        ("private-invalid-url", "origin is not a valid URL"),
        (
            "https://private-user:private-password@auth.example:private-port",
            "origin is not a valid URL",
        ),
        (
            "ftp://private-user:private-password@auth.example/private-path?token=private-token",
            "only http/https origins are allowed",
        ),
        (
            "https://private-user:private-password@auth.example",
            "credentials are not allowed in an origin",
        ),
        (
            "https://auth.example/private-path",
            "origin only: no path, query, or fragment",
        ),
        (
            "https://auth.example?token=private-token",
            "origin only: no path, query, or fragment",
        ),
        (
            "https://auth.example#private-fragment",
            "origin only: no path, query, or fragment",
        ),
        (
            "https://private-user:private-password@auth.example?token=private-token",
            "origin only: no path, query, or fragment",
        ),
    ] {
        assert_eq!(normalize_callout_origin(raw).unwrap_err(), expected);
        assert!(normalize_callout_origin_quiet(raw).is_none());
        let mut config = file_config();
        config.callout_allowlist = vec![raw.to_string()];
        assert_eq!(config.validate().unwrap_err(), format!("calloutAllowlist: {expected}"));
        let original = serde_json::to_value(&config).unwrap();
        let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "wasm-redaction", "namespace": "default"},
            "spec": {"requestPlugins": [{"type": "Wasm", "config": original}]}
        }))
        .unwrap();
        let errors = resource.validate_plugins();
        assert_eq!(
            errors,
            vec![format!("requestPlugins[0] (Wasm): calloutAllowlist: {expected}")]
        );
        assert_eq!(serde_json::to_value(&config).unwrap(), original);
        assert_eq!(
            serde_json::to_value(&resource).unwrap()["spec"]["requestPlugins"][0]["config"]["calloutAllowlist"][0],
            raw
        );
    }
}

#[test]
fn request_admission_and_explicit_response_hooks_validate() {
    let config: WasmConfig = serde_json::from_value(serde_json::json!({
        "source": {"file": "demo.wasm"},
        "response": {"headers": true, "body": true, "trailers": true}
    }))
    .unwrap();
    assert_eq!(config.max_active_requests, 32);
    assert!(config.response.enabled());
    assert!(config.validate().is_ok());
    assert!(serde_json::from_value::<WasmConfig>(serde_json::json!({
        "source": {"file": "demo.wasm"}, "response": {"header": true}
    }))
    .is_err());
    for (limit, valid) in [(0, false), (1, true), (64, true), (65, false)] {
        let mut config = file_config();
        config.max_active_requests = limit;
        assert_eq!(config.validate().is_ok(), valid);
    }
}
