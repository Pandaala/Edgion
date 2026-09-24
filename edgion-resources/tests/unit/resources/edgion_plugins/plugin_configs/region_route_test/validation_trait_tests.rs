use super::*;
use crate::resources::edgion_plugins::PluginValidation;

fn region(endpoint: &str) -> RegionDef {
    RegionDef {
        name: "region".to_string(),
        hash_range: [0, 999],
        backend_endpoint: endpoint.to_string(),
        tls: false,
        failover_to: None,
    }
}

#[test]
fn endpoint_grammar_and_default_ports() {
    for (endpoint, expected) in [
        ("backend", ("backend", 80)),
        ("Backend.EXAMPLE.", ("Backend.EXAMPLE.", 80)),
        ("192.0.2.1:65535", ("192.0.2.1", 65535)),
        ("[2001:db8::1]", ("2001:db8::1", 80)),
        ("[::ffff:127.0.0.1]:8080", ("::ffff:127.0.0.1", 8080)),
    ] {
        assert_eq!(region(endpoint).parse_endpoint(), Ok(expected), "{endpoint}");
    }
    let mut tls = region("backend");
    tls.tls = true;
    assert_eq!(tls.parse_endpoint(), Ok(("backend", 443)));
    for endpoint in [
        "",
        " ",
        "http://backend",
        "backend/path",
        "user@backend",
        "backend?x",
        "backend#x",
        "backend:",
        "backend:abc",
        "backend:0",
        "backend:65536",
        "backend:+80",
        "backend:80:90",
        "-bad",
        "bad-",
        "a..b",
        "a_b",
        "[::1",
        "[bad]:80",
        "[::1]suffix",
        "::1",
        "127.1",
        "999.1.1.1",
    ] {
        assert!(region(endpoint).parse_endpoint().is_err(), "{endpoint}");
    }
}

#[test]
fn base_table_cannot_depend_on_an_overlay_for_safety() {
    let mut config = RegionRouteConfig::default();
    assert!(config.validation_error().is_none());
    config.route_rules = vec![RouteRule::RouteByHashRange];
    assert!(config.validation_error().unwrap().contains("empty"));
    config.override_ref = Some(serde_json::from_str(r#"{"name":"overlay"}"#).unwrap());
    assert!(config.validation_error().unwrap().contains("empty"));
    config.regions = vec![region("backend")];
    assert!(config.validation_error().is_none());
    config.regions[0].hash_range = [0, 1000];
    assert!(config.validation_error().unwrap().contains("modulo 1000"));
    config.route_rules.clear();
    assert!(
        config.validation_error().is_some(),
        "a non-empty inert table still must be structurally valid"
    );
}

#[test]
fn validation_error_rejects_modulo_below_two_without_prior_validate() {
    for modulo in [0, 1] {
        let config = RegionRouteConfig {
            hash_calc: Some(HashCalcConfig {
                algorithm: HashAlgorithm::default(),
                modulo,
                write_back: Vec::new(),
            }),
            ..RegionRouteConfig::default()
        };

        assert_eq!(
            config.validation_error().as_deref(),
            Some("hashCalc.modulo must be at least 2")
        );
    }
}

#[test]
fn validation_error_accepts_minimum_and_default_modulo_without_prior_validate() {
    let minimum = RegionRouteConfig {
        hash_calc: Some(HashCalcConfig {
            algorithm: HashAlgorithm::default(),
            modulo: 2,
            write_back: Vec::new(),
        }),
        ..RegionRouteConfig::default()
    };

    assert_eq!(minimum.validation_error(), None);
    assert_eq!(RegionRouteConfig::default().validation_error(), None);
}
