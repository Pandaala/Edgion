use super::*;
use crate::resources::edgion_plugins::EdgionPlugin;

#[test]
fn mirror_capacity_boundaries_and_defensive_fallback() {
    let defaults = MirrorTuning::default().resolve_limits().unwrap();
    assert_eq!(
        (
            defaults.max_concurrent,
            defaults.max_buffered_chunks,
            defaults.max_buffered_bytes
        ),
        (1024, 5, 1024 * 1024)
    );
    for value in [1, MirrorTuning::MAX_CONCURRENT] {
        let tuning = MirrorTuning {
            max_concurrent: value,
            ..Default::default()
        };
        assert_eq!(tuning.resolve_limits().unwrap().max_concurrent, value);
    }
    for value in [0, MirrorTuning::MAX_CONCURRENT + 1, usize::MAX] {
        let mut tuning = MirrorTuning {
            max_concurrent: value,
            max_buffered_chunks: 17,
            ..Default::default()
        };
        assert!(tuning.resolve_limits().unwrap_err().contains("maxConcurrent"));
        assert_eq!(tuning.normalize_limits().max_concurrent, defaults.max_concurrent);
        assert_eq!(tuning.max_buffered_chunks, 17);
    }
    for value in [1, MirrorTuning::MAX_BUFFERED_CHUNKS] {
        let tuning = MirrorTuning {
            max_buffered_chunks: value,
            ..Default::default()
        };
        assert_eq!(tuning.resolve_limits().unwrap().max_buffered_chunks, value);
    }
    for value in [0, MirrorTuning::MAX_BUFFERED_CHUNKS + 1, usize::MAX] {
        let mut tuning = MirrorTuning {
            max_buffered_chunks: value,
            max_concurrent: 17,
            ..Default::default()
        };
        assert!(tuning.resolve_limits().unwrap_err().contains("maxBufferedChunks"));
        assert_eq!(
            tuning.normalize_limits().max_buffered_chunks,
            defaults.max_buffered_chunks
        );
        assert_eq!(tuning.max_concurrent, 17);
    }
}

#[test]
fn mirror_byte_size_boundaries_and_plugin_validation() {
    for (value, expected) in [
        ("1B".to_string(), 1),
        ("1.5KiB".to_string(), 1536),
        (
            MirrorTuning::MAX_BUFFERED_BYTES.to_string(),
            MirrorTuning::MAX_BUFFERED_BYTES,
        ),
    ] {
        let tuning = MirrorTuning {
            max_buffered_bytes: value,
            ..Default::default()
        };
        assert_eq!(tuning.resolve_limits().unwrap().max_buffered_bytes, expected);
    }
    let beyond_platform_limit = MirrorTuning {
        max_buffered_bytes: (MirrorTuning::MAX_BUFFERED_BYTES + 1).to_string(),
        ..Default::default()
    };
    assert!(beyond_platform_limit.resolve_limits().is_err());
    for value in [
        "0",
        "-1MiB",
        "2GiB",
        "1073741825",
        "garbage",
        "NaN",
        "1e400",
        "99999999999999999999999g",
    ] {
        let mut tuning = MirrorTuning {
            max_buffered_bytes: value.into(),
            ..Default::default()
        };
        assert!(tuning.resolve_limits().unwrap_err().contains("maxBufferedBytes"));
        assert_eq!(tuning.normalize_limits().max_buffered_bytes, 1024 * 1024);
        assert_eq!(tuning.max_buffered_bytes, "1MiB");
    }
    for field in ["maxConcurrent", "maxBufferedChunks", "maxBufferedBytes"] {
        let mut config = serde_json::json!({ "backendRef": { "name": "mirror", "port": 8080 } });
        config[field] = if field == "maxBufferedBytes" {
            "0".into()
        } else {
            0.into()
        };
        let plugin = EdgionPlugin::RequestMirror(serde_json::from_value(config).unwrap());
        assert!(plugin.validation_error().unwrap().contains(field));
    }
    let config: RequestMirrorPluginConfig = serde_json::from_value(serde_json::json!({
        "backendRef": {"name": "mirror", "port": 8080}, "maxBufferedBytes": "2MiB"
    }))
    .unwrap();
    assert_eq!(
        config.tuning.resolve_limits().unwrap().max_buffered_bytes,
        2 * 1024 * 1024
    );
    assert_eq!(serde_json::to_value(config).unwrap()["maxBufferedBytes"], "2MiB");
}

#[test]
fn mirror_standard_field_validation_matrix() {
    let validate = |config| {
        let config: RequestMirrorPluginConfig = serde_json::from_value(config).unwrap();
        config.validation_error()
    };
    for valid in [
        serde_json::json!({"backendRef": {"name": "mirror", "port": 1}}),
        serde_json::json!({"backendRef": {"name": "mirror", "port": 65535}, "fraction": {"numerator": 0, "denominator": 1}}),
        serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "fraction": {"numerator": 100}}),
        serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "percent": 0}),
        serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "percent": 100}),
    ] {
        assert_eq!(validate(valid), None);
    }

    for (invalid, expected) in [
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "fraction": {"numerator": 1}, "percent": 50}),
            "only one of fraction or percent",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "fraction": {"numerator": 0, "denominator": 0}}),
            "fraction.denominator",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "fraction": {"numerator": -1}}),
            "fraction.numerator",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "fraction": {"numerator": 2, "denominator": 1}}),
            "fraction.numerator",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "percent": -1}),
            "percent",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}, "percent": 101}),
            "percent",
        ),
        (
            serde_json::json!({"backendRef": {"name": "", "port": 8080}}),
            "backendRef.name",
        ),
        (
            serde_json::json!({"backendRef": {"group": "example.io", "kind": "Foo", "name": "mirror", "port": 8080}}),
            "core Service",
        ),
        (
            serde_json::json!({"backendRef": {"kind": "ConfigMap", "name": "mirror", "port": 8080}}),
            "core Service",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror"}}),
            "backendRef.port is required",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 0}}),
            "backendRef.port",
        ),
        (
            serde_json::json!({"backendRef": {"name": "mirror", "port": 65536}}),
            "backendRef.port",
        ),
    ] {
        let error = validate(invalid).expect("invalid mirror config");
        assert!(error.contains(expected), "expected {expected:?} in {error:?}");
    }
}
