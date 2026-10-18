use super::*;

#[test]
fn test_default_config() {
    let cors = CorsConfig::default();

    // Security-first defaults
    assert_eq!(cors.allow_origins, ""); // No origins allowed by default
    assert_eq!(cors.allow_methods, "GET,HEAD,OPTIONS"); // Only safe methods
    assert_eq!(
        cors.allow_headers,
        "Accept,Accept-Language,Content-Language,Content-Type,Range"
    );
    assert_eq!(cors.expose_headers, ""); // No extra headers exposed
    assert!(!cors.allow_credentials);
    assert_eq!(cors.max_age, None);
    assert!(!cors.preflight_continue);
    assert!(!cors.allow_private_network);
}

#[test]
fn test_default_config_rejects_all_origins() {
    let mut cors = CorsConfig::default();
    cors.validate_and_init().unwrap();

    // Default empty origin should reject all requests
    assert!(!cors.is_origin_allowed("https://example.com"));
    assert!(!cors.is_origin_allowed("https://trusted.com"));
    assert!(!cors.is_origin_allowed("http://localhost:3000"));
}

#[test]
fn test_credentials_with_wildcard_fails() {
    let mut config = CorsConfig {
        allow_origins: "*".to_string(),
        allow_credentials: true,
        ..Default::default()
    };

    let result = config.validate_and_init();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("allow_credentials"));
}

#[test]
fn test_comma_separated_origins() {
    let mut cors = CorsConfig {
        allow_origins: "https://a.com,https://b.com,https://c.com".to_string(),
        allow_methods: "GET,POST".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://a.com"));
    assert!(cors.is_origin_allowed("https://b.com"));
    assert!(cors.is_origin_allowed("https://c.com"));
    assert!(!cors.is_origin_allowed("https://d.com"));
}

#[test]
fn test_null_origin_rejected() {
    let mut cors = CorsConfig {
        allow_origins: "*".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(!cors.is_origin_allowed("null"));
}

#[test]
fn test_regex_matching() {
    let mut cors = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        allow_origins_by_regex: Some(vec![
            "^https://.*\\.test\\.com$".to_string(),
            "^http://localhost:[0-9]+$".to_string(),
        ]),
        allow_methods: "GET,POST".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://a.test.com"));
    assert!(cors.is_origin_allowed("https://b.test.com"));
    assert!(cors.is_origin_allowed("http://localhost:3000"));
    assert!(cors.is_origin_allowed("http://localhost:8080"));
    assert!(cors.is_origin_allowed("https://example.com"));
    assert!(!cors.is_origin_allowed("https://test.com"));
    assert!(!cors.is_origin_allowed("http://localhost"));
}

#[test]
fn test_force_wildcard() {
    let mut cors = CorsConfig {
        allow_origins: "**".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://any.com"));
    assert!(cors.is_origin_allowed("http://localhost:3000"));
}

#[test]
fn test_methods_expansion() {
    let cors = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        allow_methods: "**".to_string(),
        ..Default::default()
    };
    let methods = cors.get_allow_methods();

    assert!(methods.contains("GET"));
    assert!(methods.contains("POST"));
    assert!(methods.contains("PUT"));
    assert!(methods.contains("DELETE"));
    assert!(methods.contains("PATCH"));
}

#[test]
fn test_headers_reflection() {
    let cors = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        allow_headers: "**".to_string(),
        ..Default::default()
    };
    let headers = cors.get_allow_headers(|| Some("Content-Type,X-Custom".to_string()));

    assert_eq!(headers, "Content-Type,X-Custom");
}

#[test]
fn test_timing_origin_matching() {
    let mut cors = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        timing_allow_origins: Some("https://analytics.com".to_string()),
        timing_allow_origins_by_regex: Some(vec!["^https://.*\\.cdn\\.com$".to_string()]),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_timing_origin_allowed("https://analytics.com"));
    assert!(cors.is_timing_origin_allowed("https://a.cdn.com"));
    assert!(!cors.is_timing_origin_allowed("https://other.com"));
}

#[test]
fn test_vary_header_logic() {
    let cors1 = CorsConfig {
        allow_origins: "*".to_string(),
        ..Default::default()
    };
    assert!(!cors1.should_add_vary_header());

    let cors2 = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        ..Default::default()
    };
    assert!(cors2.should_add_vary_header());
}

#[test]
fn test_subdomain_wildcard() {
    let mut cors = CorsConfig {
        allow_origins: "*.example.com".to_string(),
        allow_methods: "GET,POST".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://a.example.com"));
    assert!(cors.is_origin_allowed("https://b.example.com"));
    // HTTPS-only: the `*.domain` shorthand does NOT match http:// origins.
    // To allow http (e.g. local dev), operators must use `allow_origins_by_regex`.
    assert!(!cors.is_origin_allowed("http://test.example.com"));
    assert!(!cors.is_origin_allowed("https://example.com")); // Root domain not included
    assert!(!cors.is_origin_allowed("https://other.com"));
}

#[test]
fn test_subdomain_wildcard_http_origins_via_regex() {
    // Explicit escape hatch for operators who need http:// matching on a subdomain.
    let mut cors = CorsConfig {
        allow_origins: "https://app.example.com".to_string(),
        allow_origins_by_regex: Some(vec![r"^https?://[^/]+\.example\.com$".to_string()]),
        allow_methods: "GET,POST".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://a.example.com"));
    assert!(cors.is_origin_allowed("http://legacy.example.com"));
    assert!(!cors.is_origin_allowed("https://other.com"));
}

#[test]
fn test_comma_separated_with_wildcard() {
    let mut cors = CorsConfig {
        allow_origins: "https://exact.com,*.example.com,https://another.com".to_string(),
        allow_methods: "GET,POST".to_string(),
        ..Default::default()
    };
    cors.validate_and_init().unwrap();

    assert!(cors.is_origin_allowed("https://exact.com"));
    assert!(cors.is_origin_allowed("https://sub.example.com"));
    assert!(cors.is_origin_allowed("https://another.com"));
    assert!(!cors.is_origin_allowed("https://example.com"));
}

#[test]
fn test_preflight_continue_config() {
    let cors1 = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        preflight_continue: true,
        ..Default::default()
    };
    assert!(cors1.preflight_continue);

    let cors2 = CorsConfig {
        allow_origins: "https://example.com".to_string(),
        preflight_continue: false,
        ..Default::default()
    };
    assert!(!cors2.preflight_continue);

    let cors3 = CorsConfig::default();
    assert!(!cors3.preflight_continue);
}

#[test]
fn test_private_network_config() {
    let cors1 = CorsConfig {
        allow_private_network: true,
        ..Default::default()
    };
    assert!(cors1.allow_private_network);

    let cors2 = CorsConfig::default();
    assert!(!cors2.allow_private_network);
}

mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    #[test]
    fn token_lists_preserve_valid_extension_tokens_and_formatting() {
        // These are lists of tokens, not a fixed method/header enum. Empty members
        // and OWS follow HTTP list parsing; validation must not rewrite the value.
        let long_token = "X".repeat(300);
        for value in [
            "",
            " \t",
            ", ,\t,",
            "GET,POST",
            "PROPFIND,REPORT,custom-Method",
            "Content-Type,X-Custom_Header",
            "GET, GET,\tPOST ,",
            "!#$%&'*+-.^_`|~0123456789AZaz",
            "*",
            "**",
            "*,X-Custom",
            long_token.as_str(),
        ] {
            for field in ["allowMethods", "allowHeaders", "exposeHeaders"] {
                let mut raw = serde_json::json!({"allowOrigins": "https://example.com"});
                raw[field] = serde_json::json!(value);
                let mut config: CorsConfig = serde_json::from_value(raw).unwrap();
                let before = serde_json::to_value(&config).unwrap();
                assert_eq!(config.validation_error(), None, "{field}={value:?}");
                config.validate_and_init().unwrap();
                assert_eq!(serde_json::to_value(&config).unwrap(), before);
            }
        }
    }

    #[test]
    fn invalid_token_lists_fail_both_entry_points_before_initialization() {
        for value in [
            "GET POST",
            "X-Name: value",
            "X/Name",
            "\"X-Name\"",
            "X;Name",
            "X=Name",
            "X\tName",
            "X\r\nName",
            "X\n",
            "X\0Name",
            "X\x7fName",
            "X-café",
        ] {
            for field in ["allowMethods", "allowHeaders", "exposeHeaders"] {
                let mut raw = serde_json::json!({"allowOrigins": "https://a.example,https://b.example"});
                raw[field] = serde_json::json!(format!("Valid, {value}"));
                let mut config: CorsConfig = serde_json::from_value(raw).unwrap();
                let error = config.validation_error().expect("invalid list must fail");
                assert_eq!(
                    error,
                    format!("{field} list item 2 must be an HTTP token; separate names with commas")
                );
                assert_eq!(config.validate_and_init(), Err(error));
                assert!(config.origins_cache.is_none());
            }
        }
    }

    #[test]
    fn credentialed_lists_accept_extensions_but_preserve_wildcard_rejection() {
        let mut config: CorsConfig = serde_json::from_value(serde_json::json!({
            "allowOrigins": "https://example.com",
            "allowCredentials": true,
            "allowMethods": "PROPFIND, custom-Method",
            "allowHeaders": "Authorization, X-Custom_Header",
            "exposeHeaders": "X-Count, X-Count,"
        }))
        .unwrap();
        assert_eq!(config.validation_error(), None);
        config.validate_and_init().unwrap();

        for field in ["allowMethods", "allowHeaders", "exposeHeaders"] {
            for wildcard in ["*", "**", "X-*", "X, *"] {
                let mut raw = serde_json::to_value(&config).unwrap();
                raw[field] = serde_json::json!(wildcard);
                let mut invalid: CorsConfig = serde_json::from_value(raw).unwrap();
                let error = invalid.validation_error().unwrap();
                assert!(error.contains("allow_credentials"));
                assert_eq!(invalid.validate_and_init(), Err(error));
            }
        }
    }

    #[test]
    fn test_default_config_returns_none() {
        let cors = CorsConfig::default();
        assert_eq!(cors.validation_error(), None);
    }

    #[test]
    fn test_wildcard_origins_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(
            err.unwrap().contains("allow_origins when allow_credentials is true"),
            "error message should mention allow_origins and allow_credentials"
        );
    }

    #[test]
    fn test_double_wildcard_origins_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "**".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        assert!(cors.validation_error().is_some());
    }

    #[test]
    fn test_wildcard_methods_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_methods: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("allow_methods when allow_credentials is true"));
    }

    #[test]
    fn test_wildcard_headers_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_headers: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("allow_headers when allow_credentials is true"));
    }

    #[test]
    fn test_wildcard_expose_headers_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            expose_headers: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("expose_headers when allow_credentials is true"));
    }

    #[test]
    fn test_wildcard_timing_origins_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            timing_allow_origins: Some("*".to_string()),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err
            .unwrap()
            .contains("timing_allow_origins when allow_credentials is true"));
    }

    #[test]
    fn test_regex_origins_with_credentials_is_error() {
        // A catch-all regex bypasses the literal '*' guard but still reproduces the
        // prohibited ACAO:<origin> + ACAC:true response at runtime, so it must be rejected.
        let cors = CorsConfig {
            allow_origins_by_regex: Some(vec![".*".to_string()]),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err
            .unwrap()
            .contains("allow_origins_by_regex when allow_credentials is true"));

        let mut cors2 = cors.clone();
        assert!(cors2.validate_and_init().is_err());
    }

    #[test]
    fn test_timing_regex_origins_with_credentials_is_error() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            timing_allow_origins_by_regex: Some(vec![".*".to_string()]),
            allow_credentials: true,
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert!(err
            .unwrap()
            .contains("timing_allow_origins_by_regex when allow_credentials is true"));

        let mut cors2 = cors.clone();
        assert!(cors2.validate_and_init().is_err());
    }

    #[test]
    fn test_valid_config_with_credentials_returns_none() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_methods: "GET,POST".to_string(),
            allow_headers: "Content-Type,Authorization".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        assert_eq!(cors.validation_error(), None);
    }

    #[test]
    fn test_invalid_regex_in_allow_origins_by_regex() {
        let cors = CorsConfig {
            allow_origins_by_regex: Some(vec!["[invalid regex(".to_string()]),
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        let msg = err.unwrap();
        assert_eq!(msg, "Invalid regex in allow_origins_by_regex item 1");
        assert!(!msg.contains("[invalid regex("));
    }

    #[test]
    fn test_invalid_regex_in_timing_allow_origins_by_regex() {
        let cors = CorsConfig {
            timing_allow_origins_by_regex: Some(vec!["(*bad".to_string()]),
            ..Default::default()
        };
        let err = cors.validation_error();
        assert!(err.is_some());
        assert_eq!(err.unwrap(), "Invalid regex in timing_allow_origins_by_regex item 1");
    }

    #[test]
    fn test_duplicate_capture_names_rejected_before_origin_matcher_build() {
        let patterns = vec![
            r"(?P<sub>[a-z]+)\.example\.com".to_string(),
            r"(?P<sub>[a-z]+)\.example\.org".to_string(),
        ];
        for timing in [false, true] {
            let mut cors = if timing {
                CorsConfig {
                    timing_allow_origins_by_regex: Some(patterns.clone()),
                    ..Default::default()
                }
            } else {
                CorsConfig {
                    allow_origins: "*.other.example.com".to_string(),
                    allow_origins_by_regex: Some(patterns.clone()),
                    ..Default::default()
                }
            };
            let field = if timing {
                "timing_allow_origins_by_regex"
            } else {
                "allow_origins_by_regex"
            };
            let expected = format!("Invalid regex in {field} item 2");
            assert_eq!(cors.validation_error(), Some(expected.clone()));
            assert_eq!(cors.validate_and_init(), Err(expected));
        }
    }

    #[test]
    fn test_valid_regex_patterns_return_none() {
        let cors = CorsConfig {
            allow_origins_by_regex: Some(vec![
                r"^https://.*\.example\.com$".to_string(),
                r"^http://localhost:[0-9]+$".to_string(),
            ]),
            timing_allow_origins_by_regex: Some(vec![r"^https://cdn\.example\.com$".to_string()]),
            ..Default::default()
        };
        assert_eq!(cors.validation_error(), None);
    }

    #[test]
    fn test_error_message_matches_validate_and_init() {
        // Ensure validation_error() and validate_and_init() return the same message
        // for the wildcard+credentials conflict so users get consistent errors.
        let cors = CorsConfig {
            allow_origins: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let trait_msg = cors.validation_error().expect("should be Some");

        let mut cors2 = CorsConfig {
            allow_origins: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };
        let init_msg = cors2.validate_and_init().expect_err("should be Err");

        assert_eq!(trait_msg, init_msg);
    }
}

#[test]
fn allow_headers_materializes_only_dynamic_reflection() {
    for configured in ["*", "X-One,X-Two"] {
        let config = CorsConfig {
            allow_headers: configured.to_string(),
            ..Default::default()
        };
        let value = config.get_allow_headers(|| panic!("static headers must not read request values"));
        assert_eq!(value, configured);
        assert_eq!(value.as_ptr(), config.allow_headers.as_ptr());
    }
    let config = CorsConfig {
        allow_headers: "**".to_string(),
        ..Default::default()
    };
    for input in [String::new(), "X-One,X-Two".to_string()] {
        let pointer = input.as_ptr();
        let expected = input.clone();
        let mut calls = 0;
        let value = config.get_allow_headers(|| {
            calls += 1;
            Some(input)
        });
        assert_eq!(calls, 1);
        assert!(matches!(value, Cow::Owned(_)));
        assert_eq!(value, expected);
        assert_eq!(value.as_ptr(), pointer);
    }
    assert_eq!(config.get_allow_headers(|| None), "*");
}
