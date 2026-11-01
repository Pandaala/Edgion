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
fn failure_policy_defaults_and_strict_structure() {
    let config: RegionRouteConfig = serde_json::from_value(serde_json::json!({
        "clusterRegionRef": {"name":"local"}
    }))
    .unwrap();
    assert!(config.fail_open);
    assert_eq!(config.status_on_error, 503);
    assert!(config.validation_error().is_none());
    for value in [
        serde_json::json!({}),
        serde_json::json!({"clusterRegionRef":{"name":"local"},"failOpen":"yes"}),
        serde_json::json!({"clusterRegionRef":{"name":"local"},"onConfigError":{}}),
        serde_json::json!({"clusterRegionRef":{"name":"local"},"myRegion":"local"}),
        serde_json::json!({"clusterRegionRef":{"name":"local"},"regions":[]}),
    ] {
        assert!(serde_json::from_value::<RegionRouteConfig>(value).is_err());
    }
    for status in [0, 200, 399, 600] {
        let mut config = config.clone();
        config.status_on_error = status;
        assert!(config.structural_validation_error().is_some());
    }
    for status in [400, 500, 599] {
        let mut config = config.clone();
        config.status_on_error = status;
        assert!(config.structural_validation_error().is_none());
    }
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
