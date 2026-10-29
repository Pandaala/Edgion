use super::*;

#[test]
fn curves_preserve_order_and_reject_invalid_lists() {
    assert_eq!(
        parse_ecdh_curves(["P-384", "X25519", "P-256", "P-521"]).unwrap(),
        vec![EcdhCurve::P384, EcdhCurve::X25519, EcdhCurve::P256, EcdhCurve::P521]
    );
    for names in [
        vec![],
        vec![""],
        vec!["P-256", "P-256"],
        vec!["prime256v1"],
        vec![" P-256"],
        vec!["P-256", "P-384", "P-521", "X25519", "P-256"],
    ] {
        assert!(parse_ecdh_curves(names).is_err());
    }
}

#[test]
fn absent_curves_use_defaults_while_explicit_empty_is_invalid() {
    assert_eq!(DownstreamTlsConfig::default().curves().unwrap(), None);
    let explicit = DownstreamTlsConfig {
        ecdh_curves: Some(Vec::new()),
        ..Default::default()
    };
    assert!(explicit.curves().is_err());
}

#[test]
fn tls_defaults_round_trip_and_cipher_presence_are_explicit() {
    let config: DownstreamTlsConfig = serde_json::from_value(serde_json::json!({
        "minTlsVersion": "TLS1_3", "ciphers": [], "ecdhCurves": ["P-256"]
    }))
    .unwrap();
    assert_eq!(config.min_tls_version, Some(TlsVersion::Tls13));
    assert_eq!(config.ciphers.as_deref(), Some([].as_slice()));
    assert!(config.validate().is_ok());
    let wire = serde_json::to_value(&config).unwrap();
    assert_eq!(wire["minTlsVersion"], "TLS1_3");
    assert_eq!(wire["ciphers"], serde_json::json!([]));
    assert!(DownstreamTlsConfig::default().ciphers.is_none());
    let invalid = DownstreamTlsConfig {
        ciphers: Some(vec!["DEFAULT\0EXTRA".into()]),
        ..Default::default()
    };
    assert_eq!(invalid.validate(), Err("ciphers must not contain NUL bytes"));
    // Backend expressions are intentionally not parsed in the schema crate.
    assert!(DownstreamTlsConfig {
        ciphers: Some(vec!["HIGH:!aNULL".into()]),
        ..Default::default()
    }
    .validate()
    .is_ok());
}
