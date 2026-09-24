use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

#[test]
fn credentialed_expose_headers_validation_and_wire_values() {
    for credentials in [None, Some(false), Some(true)] {
        for names in [vec![], vec!["*"], vec!["X-Request-Id"], vec!["*", "X-Request-Id"]] {
            let config = HTTPCORSFilter {
                allow_origins: Some(vec!["https://app.example".into()]),
                allow_credentials: credentials,
                allow_methods: None,
                allow_headers: None,
                expose_headers: Some(names.iter().map(|name| name.to_string()).collect()),
                max_age: 5,
            };
            let result = GapiCorsFilter::try_new(config);
            if credentials == Some(true) && names.contains(&"*") {
                assert!(result.unwrap_err().contains("exposeHeaders"));
                continue;
            }
            let filter = result.unwrap();
            for preflight in [false, true] {
                let mut session = MockPluginSession::new();
                let headers = filter.build_cors_headers(&mut session, "https://app.example", preflight);
                let exposed = headers
                    .iter()
                    .find(|(name, _)| *name == "Access-Control-Expose-Headers");
                assert_eq!(
                    exposed.map(|(_, value)| value.clone()),
                    (!names.is_empty()).then(|| names.join(", "))
                );
            }
        }
    }
}

#[test]
fn max_age_default_is_emitted_and_explicit_zero_is_rejected() {
    let config: HTTPCORSFilter = serde_json::from_value(serde_json::json!({
        "allowOrigins": ["https://app.example"]
    }))
    .expect("default maxAge CORS fixture must deserialize");
    let filter = GapiCorsFilter::try_new(config).expect("default maxAge must be valid");
    let mut session = MockPluginSession::new();
    let headers = filter.build_cors_headers(&mut session, "https://app.example", true);
    assert!(headers
        .iter()
        .any(|(name, value)| *name == "Access-Control-Max-Age" && value == "5"));

    let zero: HTTPCORSFilter =
        serde_json::from_value(serde_json::json!({"maxAge": 0})).expect("zero maxAge CORS fixture must deserialize");
    assert!(GapiCorsFilter::try_new(zero)
        .unwrap_err()
        .contains("maxAge must be between 1"));
}

#[test]
fn test_origin_match_exact() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://app.example".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://app.example"));
    assert!(!filter.is_origin_allowed("https://evil.com"));
    assert!(!filter.is_origin_allowed("http://app.example"));
}

#[test]
fn test_origin_match_wildcard() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["*".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://anything.com"));
    assert!(filter.is_origin_allowed("http://localhost:3000"));
}

#[test]
fn test_origin_match_subdomain_wildcard() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://*.example.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://app.example.com"));
    assert!(filter.is_origin_allowed("https://foo.bar.example.com"));
    assert!(!filter.is_origin_allowed("https://example.com"));
    assert!(!filter.is_origin_allowed("https://evil.com"));
}

#[test]
fn test_no_origins_rejects_all() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: None,
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(!filter.is_origin_allowed("https://anything.com"));
}

#[test]
fn maximum_header_set_stays_inline_and_preserves_order() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://app.example".to_string()]),
        allow_credentials: Some(true),
        allow_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
        allow_headers: Some(vec!["Content-Type".to_string(), "X-Trace-Id".to_string()]),
        expose_headers: Some(vec!["X-Request-Id".to_string()]),
        max_age: 3600,
    })
    .expect("maximum-header CORS test configuration must be valid");
    let mut session = MockPluginSession::new();

    let headers = filter.build_cors_headers(&mut session, "https://app.example", true);

    assert_eq!(headers.len(), 6);
    assert!(!headers.spilled());
    assert_eq!(
        headers.as_slice(),
        [
            ("Access-Control-Allow-Origin", "https://app.example".to_string()),
            ("Access-Control-Allow-Credentials", "true".to_string()),
            ("Access-Control-Allow-Methods", "GET, POST".to_string()),
            ("Access-Control-Allow-Headers", "Content-Type, X-Trace-Id".to_string()),
            ("Access-Control-Expose-Headers", "X-Request-Id".to_string()),
            ("Access-Control-Max-Age", "3600".to_string()),
        ]
        .as_slice()
    );
}

#[test]
fn test_split_origin_port() {
    let (host, port) = split_origin_port("https://example.com:8443").unwrap();
    assert_eq!(host, "https://example.com");
    assert_eq!(port, ":8443");

    let (host, port) = split_origin_port("https://example.com").unwrap();
    assert_eq!(host, "https://example.com");
    assert_eq!(port, "");

    let (host, port) = split_origin_port("http://localhost:3000").unwrap();
    assert_eq!(host, "http://localhost");
    assert_eq!(port, ":3000");
}

#[test]
fn test_v15_origin_validation_accepts_valid() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec![
            "https://example.com".to_string(),
            "https://example.com:8443".to_string(),
            "http://localhost:3000".to_string(),
            "https://*.example.com".to_string(),
        ]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    assert!(GapiCorsFilter::try_new(config).is_ok());

    let wildcard: HTTPCORSFilter = serde_json::from_value(serde_json::json!({"allowOrigins": ["*"]}))
        .expect("wildcard CORS fixture must deserialize");
    assert!(GapiCorsFilter::try_new(wildcard).is_ok());
}

#[test]
fn test_v15_origin_validation_rejects_path() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com/path".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("path component"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_trailing_slash() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com/".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    assert!(GapiCorsFilter::try_new(config).is_err());
}

#[test]
fn test_v15_origin_validation_rejects_query() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com?q=1".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("query string"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_fragment() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com#sec".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("fragment"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_no_scheme() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["example.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("scheme"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_bad_wildcard() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://exam*ple.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("wildcard"), "got: {}", err);
}
