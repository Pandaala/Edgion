use super::*;
use crate::resources::edgion_plugins::hmac_match::HmacMatchConfig;

fn core_values(values: Vec<String>) -> MatchCore {
    MatchCore {
        values: Some(values),
        ..MatchCore::default()
    }
}

fn core_regex(regex: Vec<String>) -> MatchCore {
    MatchCore {
        regex: Some(regex),
        ..MatchCore::default()
    }
}

#[test]
fn test_plugin_conditions_default() {
    let conditions = PluginConditions::default();
    assert!(conditions.is_empty());
    assert!(conditions.skip.is_none());
    assert!(conditions.run.is_none());
}

#[test]
fn plugin_conditions_reject_empty_match_payloads_even_when_negated() {
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Header {
                    name: "X-Test".to_string(),
                },
                core: MatchCore::default(),
                negate: true,
                descriptor: Box::from(""),
            }),
            Condition::JwtClaimMatch(JwtClaimMatchCondition {
                claim: "sub".to_string(),
                core: MatchCore::default(),
                negate: true,
                descriptor: Box::from(""),
            }),
            Condition::MtlsMatch(MtlsMatchCondition {
                field: MtlsField::Cn,
                san_type: None,
                core: MatchCore::default(),
                negate: true,
                descriptor: Box::from(""),
            }),
        ])),
    };

    let errors = conditions
        .validate()
        .expect_err("empty match payloads must be rejected before runtime");
    assert_eq!(errors.len(), 3, "{errors:?}");
    for condition_type in ["keyMatch", "jwtClaimMatch", "mtlsMatch"] {
        assert!(
            errors.iter().any(|error| error.contains(condition_type)),
            "missing {condition_type} error: {errors:?}"
        );
    }
}

#[test]
fn plugin_conditions_reject_empty_values_even_with_valid_regex() {
    let core = || MatchCore {
        values: Some(vec![]),
        regex: Some(vec!["^allowed$".to_string()]),
        ..MatchCore::default()
    };
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Path,
                core: core(),
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::JwtClaimMatch(JwtClaimMatchCondition {
                claim: "sub".to_string(),
                core: core(),
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::MtlsMatch(MtlsMatchCondition {
                field: MtlsField::Cn,
                san_type: None,
                core: core(),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    };

    let errors = conditions
        .validate()
        .expect_err("an explicitly empty values array must always be rejected");
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(errors.iter().all(|error| error.contains("values cannot be empty")));
}

#[test]
fn plugin_conditions_reject_empty_named_key_get() {
    let conditions = PluginConditions {
        skip: Some(ConditionSet::any_of(vec![Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Ctx { name: String::new() },
            negate: false,
            descriptor: Box::from(""),
        })])),
        run: None,
    };

    let errors = conditions.validate().expect_err("empty key name must be rejected");
    assert!(errors.iter().any(|error| error.contains("keyExist.key.name")));
}

#[test]
fn plugin_conditions_reject_blank_jwt_claim_even_when_negated() {
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![Condition::JwtClaimMatch(
            JwtClaimMatchCondition {
                claim: "  \t".to_string(),
                core: core_values(vec!["admin".to_string()]),
                negate: true,
                descriptor: Box::from(""),
            },
        )])),
    };

    let errors = conditions
        .validate()
        .expect_err("blank JWT claim must be rejected before runtime");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("jwtClaimMatch.claim cannot be empty"));
}

#[test]
fn secret_match_requires_nonempty_secret_key_and_uses_camel_case_serde() {
    let missing = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretRef: { name: access-token }
"#;
    assert!(
        serde_yaml::from_str::<Condition>(missing).is_err(),
        "secretKey is a required operator field"
    );

    let blank = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretKey: "  "
secretRef: { name: access-token }
"#;
    let condition: Condition = serde_yaml::from_str(blank).expect("secretMatch shape deserializes");
    let error = condition.validate().expect_err("blank secretKey must be rejected");
    assert!(error.contains("secretMatch.secretKey cannot be empty"), "{error}");

    let valid = r#"
type: secretMatch
key: { type: header, name: X-Token }
secretKey: token
secretRef: { name: access-token }
"#;
    let condition: Condition = serde_yaml::from_str(valid).expect("secretMatch with secretKey deserializes");
    assert!(condition.validate().is_ok());
    let yaml = serde_yaml::to_string(&condition).unwrap();
    assert!(yaml.contains("secretKey: token"), "{yaml}");
}

#[test]
fn test_serialize_deserialize() {
    let conditions = PluginConditions {
        skip: Some(ConditionSet::any_of(vec![Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
        run: Some(ConditionSet::all_of(vec![Condition::Probability(
            ProbabilityCondition {
                ratio: 0.5,
                key: None,
                negate: false,
                descriptor: Box::from(""),
            },
        )])),
    };

    let yaml = serde_yaml::to_string(&conditions).unwrap();
    let deserialized: PluginConditions = serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(deserialized.skip.as_ref().unwrap().len(), 1);
    assert_eq!(deserialized.run.as_ref().unwrap().len(), 1);
}

#[test]
fn test_key_match_compile_single_regex() {
    let mut condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "User-Agent".to_string(),
        },
        core: core_regex(vec![r"^Mozilla.*".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.compile().is_ok());
    assert!(condition.core.compiled_regex.is_some());

    let regex = condition.core.compiled_regex.as_ref().unwrap();
    assert!(regex.is_match("Mozilla/5.0"));
    assert!(!regex.is_match("Chrome/100.0"));
}

#[test]
fn test_key_match_compile_multi_regex() {
    let mut condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "User-Agent".to_string(),
        },
        core: core_regex(vec![
            r"^Mozilla.*".to_string(),
            r"^Chrome.*".to_string(),
            r"(?i:^safari.*)".to_string(), // case-insensitive
        ]),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.compile().is_ok());
    assert!(condition.core.compiled_regex.is_some());

    let regex = condition.core.compiled_regex.as_ref().unwrap();
    // Should match Mozilla
    assert!(regex.is_match("Mozilla/5.0"));
    // Should match Chrome
    assert!(regex.is_match("Chrome/100.0"));
    // Should match Safari (case-insensitive)
    assert!(regex.is_match("Safari/605.1"));
    assert!(regex.is_match("safari/605.1"));
    assert!(regex.is_match("SAFARI/605.1"));
    // Should not match curl
    assert!(!regex.is_match("curl/7.64.1"));
}

#[test]
fn test_key_match_compile_hashset() {
    // Create more than 16 values to trigger HashSet compilation
    let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();

    let mut condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Test".to_string(),
        },
        core: core_values(values),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.compile().is_ok());
    assert!(condition.core.values_set.is_some());

    let set = condition.core.values_set.as_ref().unwrap();
    assert_eq!(set.len(), 20);
    assert!(set.contains("value0"));
    assert!(set.contains("value19"));
    assert!(!set.contains("value20"));
}

#[test]
fn test_key_match_no_hashset_below_threshold() {
    // Create exactly 16 values (threshold, should NOT trigger HashSet)
    let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();

    let mut condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Test".to_string(),
        },
        core: core_values(values),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.compile().is_ok());
    assert!(condition.core.values_set.is_none()); // Should NOT be compiled
}

#[test]
fn test_key_match_compile_hashset_case_insensitive_folds() {
    let mut values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();
    values.push("Production".to_string());
    let mut condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Test".to_string(),
        },
        core: MatchCore {
            values: Some(values),
            case_sensitive: false,
            ..MatchCore::default()
        },
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.compile().is_ok());
    let set = condition.core.values_set.as_ref().unwrap();
    assert!(set.contains("production"));
    assert!(!set.contains("Production"));
    assert!(condition.core.matches("PRODUCTION"));
    assert!(condition.core.matches("production"));
}

#[test]
fn test_plugin_conditions_compile_all_independent() {
    // Mix of valid/invalid across skip and run; compile() must return EVERY
    // error (not just the first) while still compiling the valid conditions
    // and clearing the failing ones' regex to prevent per-request push_err.
    //
    // Layout (order matters — errors are returned in skip → run order):
    //   skip[0] invalid (unclosed bracket)
    //   skip[1] valid
    //   skip[2] invalid (lone repeat)
    //   run[0]  valid
    //   run[1]  invalid (mismatched paren)
    let invalid_bracket = Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec!["[invalid".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    });
    let valid_skip = Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec![r"^/api/.*".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    });
    let invalid_repeat = Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec!["*foo".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    });
    let valid_run = Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec![r"^/health$".to_string()]),
        negate: true,
        descriptor: Box::from(""),
    });
    let invalid_paren = Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec!["(unclosed".to_string()]),
        negate: true,
        descriptor: Box::from(""),
    });
    let mut conds = PluginConditions {
        skip: Some(ConditionSet::any_of(vec![invalid_bracket, valid_skip, invalid_repeat])),
        run: Some(ConditionSet::all_of(vec![valid_run, invalid_paren])),
    };

    let errs = conds.compile().expect_err("should collect every regex compile error");
    assert_eq!(errs.len(), 3, "must report all 3 invalid patterns, not just the first");

    let skip: Vec<_> = conds.skip.as_ref().unwrap().iter_conditions().collect();
    // skip[0] invalid → regex cleared
    if let Condition::KeyMatch(c) = &skip[0] {
        assert!(c.core.regex.is_none(), "skip[0] regex must be cleared");
        assert!(c.core.compiled_regex.is_none());
    } else {
        panic!("expected KeyMatch at skip[0]");
    }
    // skip[1] valid → compiled
    if let Condition::KeyMatch(c) = &skip[1] {
        assert!(
            c.core.compiled_regex.is_some(),
            "skip[1] valid condition must be compiled"
        );
    } else {
        panic!("expected KeyMatch at skip[1]");
    }
    // skip[2] invalid → regex cleared
    if let Condition::KeyMatch(c) = &skip[2] {
        assert!(c.core.regex.is_none(), "skip[2] regex must be cleared");
        assert!(c.core.compiled_regex.is_none());
    } else {
        panic!("expected KeyMatch at skip[2]");
    }

    let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
    // run[0] valid → compiled
    if let Condition::KeyMatch(c) = &run[0] {
        assert!(
            c.core.compiled_regex.is_some(),
            "run[0] valid condition must be compiled"
        );
    } else {
        panic!("expected KeyMatch at run[0]");
    }
    // run[1] invalid → regex cleared
    if let Condition::KeyMatch(c) = &run[1] {
        assert!(c.core.regex.is_none(), "run[1] regex must be cleared");
        assert!(c.core.compiled_regex.is_none());
    } else {
        panic!("expected KeyMatch at run[1]");
    }
}

#[test]
fn test_plugin_conditions_validate_collects_all_errors() {
    // Two TimeRange conditions with malformed bounds across skip and run.
    // validate() must return both errors (not just the first), in skip → run order.
    let bad_after = Condition::TimeRange(TimeRangeCondition {
        after: Some("not-a-timestamp".to_string()),
        before: None,
        negate: false,
        descriptor: Box::from(""),
    });
    let bad_before = Condition::TimeRange(TimeRangeCondition {
        after: None,
        before: Some("also-bad".to_string()),
        negate: false,
        descriptor: Box::from(""),
    });
    let conds = PluginConditions {
        skip: Some(ConditionSet::any_of(vec![bad_after])),
        run: Some(ConditionSet::all_of(vec![bad_before])),
    };

    let errs = conds.validate().expect_err("should collect every validate error");
    assert_eq!(errs.len(), 2, "must report both malformed bounds, not just the first");
}

#[test]
fn test_key_get_serialization() {
    // KeyGet uses tagged enum with lower camel-case values.
    assert_eq!(
        serde_json::to_string(&KeyGet::RemoteIp).unwrap(),
        r#"{"type":"remoteIp"}"#
    );
    assert_eq!(
        serde_json::to_string(&KeyGet::Header {
            name: "X-Test".to_string()
        })
        .unwrap(),
        r#"{"type":"header","name":"X-Test"}"#
    );
    assert_eq!(serde_json::to_string(&KeyGet::Path).unwrap(), r#"{"type":"path"}"#);
}

#[test]
fn unknown_key_paths_reports_typo_inside_flattened_match_core() {
    // First production `#[serde(flatten)]` on Condition: a typo inside the
    // flattened MatchCore region must be reported against the real
    // EdgionPlugins spec schema, not a toy struct.
    let schema = crate::resources::edgion_plugins::edgion_plugins_spec_schema();
    let value = serde_json::json!({
        "requestPlugins": [{
            "type": "IpRestriction",
            "config": {"defaultAction": "allow"},
            "conditions": {
                "run": {
                    "allOf": [{
                        "type": "keyMatch",
                        "key": {"type": "header", "name": "X-Test"},
                        "values": ["ok"],
                        "valuez": "typo"
                    }]
                }
            }
        }]
    });
    let findings = crate::schema_check::unknown_key_paths(&value, schema);
    assert!(
        findings.iter().any(|p| p.contains("valuez")),
        "flattened MatchCore typo must be reported, got {findings:?}"
    );
}

#[test]
fn condition_size_stays_under_named_ceiling() {
    let measured = std::mem::size_of::<Condition>();
    eprintln!("size_of::<Condition>()={measured} CONDITION_SIZE_CEILING={CONDITION_SIZE_CEILING}");
    assert!(
        measured <= CONDITION_SIZE_CEILING,
        "size_of::<Condition>()={measured} exceeds CONDITION_SIZE_CEILING={CONDITION_SIZE_CEILING}"
    );
}

#[test]
fn diagnostic_descriptor_is_value_free_and_ordinal_bearing() {
    let secret = "super-secret-token";
    let mut conds = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Header {
                    name: "X-A".to_string(),
                },
                core: core_values(vec![secret.to_string()]),
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Header {
                    name: "X-A".to_string(),
                },
                core: core_values(vec![secret.to_string()]),
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-B".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::TimeRange(TimeRangeCondition {
                after: Some("2020-01-01T00:00:00Z".to_string()),
                before: None,
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::Probability(ProbabilityCondition {
                ratio: 0.5,
                key: None,
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::Probability(ProbabilityCondition {
                ratio: 0.5,
                key: Some(KeyGet::Header {
                    name: "X-User".to_string(),
                }),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    };
    conds.compile().expect("compile assigns descriptors");
    let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
    assert_eq!(run[0].descriptor(), "0:keyMatch:hdr:X-A");
    assert_eq!(run[1].descriptor(), "1:keyMatch:hdr:X-A");
    assert_eq!(run[2].descriptor(), "2:keyExist:hdr:X-B");
    assert_eq!(run[3].descriptor(), "3:timeRange");
    assert_eq!(run[4].descriptor(), "4:probability");
    assert_eq!(run[5].descriptor(), "5:probability:hdr:X-User");
    for c in run {
        assert!(
            !c.descriptor().contains(secret),
            "descriptor must not contain match values: {}",
            c.descriptor()
        );
    }
}

#[test]
fn empty_skip_and_run_compile_ok() {
    let mut empty = PluginConditions::default();
    empty.compile().expect("empty skip/run compile");
    let mut none_vecs = PluginConditions {
        skip: Some(ConditionSet::any_of(vec![])),
        run: Some(ConditionSet::all_of(vec![])),
    };
    none_vecs.compile().expect("empty vecs compile");
}

fn hmac_body_yaml(validate_body: bool) -> String {
    format!(
        r#"
type: hmacAuth
secretGroups:
  - name: default
    secretRefs:
      - name: hmac-credentials
algorithms: [hmac-sha256]
validateRequestBody: {validate_body}
"#
    )
}

#[test]
fn hmac_match_condition_rejects_standalone_only_fields() {
    for field in [
        "anonymous: guest",
        "realm: edge",
        "authFailureDelayMs: 1",
        "hideCredentials: true",
        "upstreamHeaderFields: [X-Consumer]",
    ] {
        let yaml = format!("type: hmacAuth\n{field}\n");
        assert!(serde_yaml::from_str::<Condition>(&yaml).is_err(), "accepted: {field}");
    }
}

#[test]
fn hmac_match_condition_redacts_admin_output_and_round_trips_conf_sync() {
    let condition = Condition::HmacAuth(HmacMatchConfig {
        resolved_credentials: Some(std::collections::HashMap::from([(
            "alice".to_string(),
            crate::resources::edgion_plugins::plugin_configs::HmacCredential {
                secret: b"policy-conf-sync-secret".to_vec(),
                headers: std::collections::HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    });

    let admin_json = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&condition).unwrap()
    };
    assert!(!admin_json.contains("policy-conf-sync-secret"), "{admin_json}");
    assert!(admin_json.contains("[redacted]"), "{admin_json}");

    let wire = serde_json::to_vec(&condition).unwrap();
    let decoded: Condition = serde_json::from_slice(&wire).unwrap();
    let Condition::HmacAuth(config) = decoded else {
        panic!("unexpected condition type")
    };
    assert_eq!(
        config
            .resolved_credentials
            .as_ref()
            .and_then(|credentials| credentials.get("alice"))
            .map(|credential| credential.secret.as_slice()),
        Some(b"policy-conf-sync-secret".as_slice())
    );
}

#[test]
fn entry_level_hmac_validate_request_body_is_rejected() {
    let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(true)).unwrap();
    let err = condition
        .validate()
        .expect_err("entry-level HMAC body must be rejected");
    assert!(err.contains("validateRequestBody"), "{err}");

    let conds = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![condition])),
    };
    let errs = conds.validate().expect_err("PluginConditions uses entry-level path");
    assert!(errs.iter().any(|e| e.contains("validateRequestBody")));
}

#[test]
fn request_async_set_accepts_hmac_validate_request_body() {
    let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(true)).unwrap();
    ConditionSet::all_of(vec![condition])
        .validate(ConditionValidationProfile::RequestAsync)
        .expect("request-async ConditionSet accepts validateRequestBody");
}

#[test]
fn hmac_fields_stay_siblings_of_type() {
    let condition: Condition = serde_yaml::from_str(&hmac_body_yaml(false)).unwrap();
    let yaml = serde_yaml::to_string(&condition).unwrap();
    assert!(yaml.contains("type: hmacAuth"), "{yaml}");
    assert!(yaml.contains("secretGroups:"), "{yaml}");
    assert!(!yaml.contains("hmac:"), "{yaml}");
    assert!(!yaml.contains("matchConfig"), "{yaml}");
}

#[test]
fn path_key_match_normalizes_exact_values_request_path_does_not() {
    let mut path_match = KeyMatchCondition {
        key: KeyGet::Path,
        core: core_values(vec!["/%61dmin".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    };
    path_match.compile().expect("path keyMatch compiles");
    assert_eq!(path_match.core.values.as_ref().unwrap()[0], "/admin");

    let mut request_path_match = KeyMatchCondition {
        key: KeyGet::RequestPath,
        core: core_values(vec!["/%61dmin".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    };
    request_path_match.compile().expect("requestPath keyMatch compiles");
    assert_eq!(request_path_match.core.values.as_ref().unwrap()[0], "/%61dmin");
}

#[test]
fn jwt_claim_and_mtls_match_compile_without_path_normalization() {
    let mut jwt = JwtClaimMatchCondition {
        claim: "sub".to_string(),
        core: core_values(vec!["/%61dmin".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    };
    jwt.compile().expect("jwtClaimMatch compiles");
    assert_eq!(jwt.core.values.as_ref().unwrap()[0], "/%61dmin");

    let mut mtls = MtlsMatchCondition {
        field: MtlsField::San,
        san_type: Some(SanType::Dns),
        core: core_values(vec!["/%61dmin".to_string()]),
        negate: false,
        descriptor: Box::from(""),
    };
    mtls.compile().expect("mtlsMatch compiles");
    assert_eq!(mtls.core.values.as_ref().unwrap()[0], "/%61dmin");
}

#[test]
fn jwt_claim_and_mtls_descriptors_are_value_free() {
    let secret = "super-secret-token";
    let mut conds = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::JwtClaimMatch(JwtClaimMatchCondition {
                claim: "sub".to_string(),
                core: core_values(vec![secret.to_string()]),
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::MtlsMatch(MtlsMatchCondition {
                field: MtlsField::San,
                san_type: None,
                core: core_values(vec![secret.to_string()]),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    };
    conds.compile().expect("compile assigns descriptors");
    let run: Vec<_> = conds.run.as_ref().unwrap().iter_conditions().collect();
    assert_eq!(run[0].descriptor(), "0:jwtClaimMatch");
    assert_eq!(run[1].descriptor(), "1:mtlsMatch");
    for c in run {
        assert!(
            !c.descriptor().contains(secret),
            "descriptor must not contain match values: {}",
            c.descriptor()
        );
    }
}

#[test]
fn hashed_match_values_preserve_ascii_folding_and_original_regex_input() {
    let mut values: Vec<String> = (0..20).map(|i| format!("value{i}")).collect();
    values.extend(["éX".into(), "".into(), "π".into()]);
    for case_sensitive in [false, true] {
        let mut hashed = MatchCore {
            values: Some(values.clone()),
            regex: Some(vec!["^REGEX-HIT$".into()]),
            case_sensitive,
            ..Default::default()
        };
        hashed.compile(false).unwrap();
        assert!(hashed.values_set.is_some());
        let mut linear = hashed.clone();
        linear.values_set = None;
        for input in [
            "value1",
            "VALUE1",
            "éx",
            "éX",
            "π",
            "",
            "missing",
            "REGEX-HIT",
            "regex-hit",
        ] {
            let expected = values.iter().any(|v| {
                if case_sensitive {
                    input == v
                } else {
                    input.eq_ignore_ascii_case(v)
                }
            }) || input == "REGEX-HIT";
            assert_eq!(hashed.matches(input), expected);
            assert_eq!(linear.matches(input), expected);
        }
    }
}
