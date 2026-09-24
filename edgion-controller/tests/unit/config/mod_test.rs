use super::*;

#[test]
fn validation_rejects_removed_controller_plugin_policy() {
    let error = serde_yaml::from_str::<EdgionControllerConfig>(
        "validation:\n  plugin_policy:\n    mode: denylist\n    plugins: []\n",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown field"), "{error}");
    assert!(error.contains("plugin_policy"), "{error}");
    assert!(error.contains("enable_reference_grant_validation"), "{error}");
}

#[test]
fn validation_preserves_reference_grant_configuration() {
    let defaults: ValidationConfig = serde_yaml::from_str("{}").unwrap();
    assert!(defaults.enable_reference_grant_validation);
    let explicit: ValidationConfig = serde_yaml::from_str("enable_reference_grant_validation: false").unwrap();
    assert!(!explicit.enable_reference_grant_validation);
    let wire = serde_json::to_value(defaults).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 1);
    assert!(wire.get("plugin_policy").is_none());
}

#[test]
fn dns_resolver_defaults_enabled_15s() {
    let cfg = DnsResolverConfig::default();
    assert!(cfg.enabled, "control-plane resolution defaults to on");
    assert_eq!(cfg.refresh_interval, "15s");
    assert!(cfg.link_sys_ref.is_none());
    assert!(cfg.servers.is_empty());
}

#[test]
fn dns_resolver_parses_from_yaml() {
    let yaml = "enabled: false\nrefresh_interval: 30s\nservers:\n  - 1.1.1.1\n  - 8.8.8.8:5353\n";
    let cfg: DnsResolverConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(!cfg.enabled);
    assert_eq!(cfg.refresh_interval, "30s");
    assert_eq!(cfg.servers, vec!["1.1.1.1".to_string(), "8.8.8.8:5353".to_string()]);
}

#[test]
fn dns_resolver_omitted_fields_take_defaults() {
    // `#[serde(default)]` on the struct → an empty map yields the full default.
    let cfg: DnsResolverConfig = serde_yaml::from_str("{}").unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.refresh_interval, "15s");
}

#[test]
fn dns_resolver_link_sys_ref_and_servers_are_mutually_exclusive() {
    let cfg = DnsResolverConfig {
        link_sys_ref: Some("dns/aliyun".to_string()),
        servers: vec!["1.1.1.1".to_string()],
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "both set must be rejected");
}

#[test]
fn dns_resolver_single_source_validates_clean() {
    let only_ref = DnsResolverConfig {
        link_sys_ref: Some("dns/aliyun".to_string()),
        ..Default::default()
    };
    assert!(only_ref.validate().is_ok());
    let only_servers = DnsResolverConfig {
        servers: vec!["1.1.1.1".to_string()],
        ..Default::default()
    };
    assert!(only_servers.validate().is_ok());
    assert!(DnsResolverConfig::default().validate().is_ok());
}

#[test]
fn dns_resolver_rejects_unparseable_refresh_interval() {
    let cfg = DnsResolverConfig {
        refresh_interval: "soon".to_string(),
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "unparseable interval must be rejected at load");
    // A parseable sub-second value passes validation (the loop clamps it up).
    let cfg = DnsResolverConfig {
        refresh_interval: "500ms".to_string(),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_to_log_config_respects_console_flag() {
    let mut cfg = EdgionControllerConfig::default();
    cfg.logging.console = Some(false);
    let log_cfg = cfg.to_log_config();
    assert!(!log_cfg.console);
}

#[test]
fn conf_dir_override_preserves_explicit_controller_identity() {
    let mut base = EdgionControllerConfig {
        conf_center: ConfCenterConfig::FileSystem(
            FileSystemConfig::default().with_controller_name("example.com/deployment-a"),
        ),
        ..Default::default()
    };
    let cli = EdgionControllerConfig {
        conf_dir: Some(std::path::PathBuf::from("replacement")),
        ..Default::default()
    };
    EdgionControllerConfig::merge(&mut base, &cli);
    let ConfCenterConfig::FileSystem(config) = base.conf_center else {
        panic!("--conf-dir must select FileSystem");
    };
    assert_eq!(config.controller_name.as_deref(), Some("example.com/deployment-a"));
    assert_eq!(config.conf_dir, std::path::PathBuf::from("replacement"));
}

#[test]
fn merge_keeps_yaml_allowlist_when_cli_omits_flag() {
    // Simulates the K8s case: allowlist from YAML, flag absent on argv (clap → empty Vec).
    let mut base = EdgionControllerConfig::default();
    base.server.allow_admin_ips = vec!["10.0.0.0/8".to_string()];
    let cli = EdgionControllerConfig::default(); // allow_admin_ips empty
    EdgionControllerConfig::merge(&mut base, &cli);
    assert_eq!(
        base.server.allow_admin_ips,
        vec!["10.0.0.0/8".to_string()],
        "an empty CLI allowlist must not clobber the YAML allowlist"
    );
}

#[test]
fn merge_cli_allowlist_overrides_when_present() {
    let mut base = EdgionControllerConfig::default();
    base.server.allow_admin_ips = vec!["10.0.0.0/8".to_string()];
    let mut cli = EdgionControllerConfig::default();
    cli.server.allow_admin_ips = vec!["192.168.0.0/16".to_string()];
    EdgionControllerConfig::merge(&mut base, &cli);
    assert_eq!(base.server.allow_admin_ips, vec!["192.168.0.0/16".to_string()]);
}

#[test]
fn test_center_client_config_parses_from_yaml() {
    let yaml = r#"
center:
  address: "http://center.example.com:50052"
  name: "ctrl-01"
  cluster: "prod-cn"
  env: [production]
  tag: [team-infra]
"#;
    let config: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
    let center = config.center.unwrap();
    assert_eq!(center.address, "http://center.example.com:50052");
    assert_eq!(center.name, "ctrl-01");
    assert_eq!(center.cluster, "prod-cn");
    assert_eq!(center.env, vec!["production"]);
}

// ── Group E: metrics_resource_abnormal defaults (2 independent, kept) ───

#[test]
fn test_metrics_resource_abnormal_defaults() {
    let yaml = "work_dir: /tmp\n";
    let cfg: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(cfg.metrics.resource_abnormal.enabled);
    assert_eq!(cfg.metrics.resource_abnormal.tick_secs, 300);
}

#[test]
fn test_metrics_resource_abnormal_explicit_values_parse() {
    let yaml = r#"
metrics:
  resource_abnormal:
    enabled: false
    tick_secs: 60
"#;
    let cfg: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(!cfg.metrics.resource_abnormal.enabled);
    assert_eq!(cfg.metrics.resource_abnormal.tick_secs, 60);
}

// ── Group E cont.: tick validation (4 → 1 table-driven) ─────────────────

#[test]
fn test_metrics_resource_abnormal_tick_scenarios() {
    struct Case {
        name: &'static str,
        enabled: bool,
        tick_secs: u64,
        expect_err_substr: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "tick 29 is too small",
            enabled: true,
            tick_secs: 29,
            expect_err_substr: Some("tick_secs"),
        },
        Case {
            name: "tick 30 is the minimum and is accepted",
            enabled: true,
            tick_secs: 30,
            expect_err_substr: None,
        },
        Case {
            name: "tick 3601 is too large",
            enabled: true,
            tick_secs: 3601,
            expect_err_substr: Some("tick_secs"),
        },
        Case {
            name: "disabled skips range check even with tick 0",
            enabled: false,
            tick_secs: 0,
            expect_err_substr: None,
        },
    ];

    for case in &cases {
        let mut cfg = EdgionControllerConfig::default();
        cfg.metrics.resource_abnormal.enabled = case.enabled;
        cfg.metrics.resource_abnormal.tick_secs = case.tick_secs;
        let result = cfg.validate_metrics();
        match case.expect_err_substr {
            None => {
                assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
            }
            Some(substr) => {
                assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                let msg = format!("{:?}", result.unwrap_err());
                assert!(
                    msg.contains(substr),
                    "case '{}': error {:?} does not contain {:?}",
                    case.name,
                    msg,
                    substr
                );
            }
        }
    }
}

// ── Group F: hard_cap validation (3 → 1 table-driven) ───────────────────

#[test]
fn test_metrics_resource_abnormal_hard_cap_scenarios() {
    struct Case {
        name: &'static str,
        hard_cap: usize,
        expect_err_substr: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "default hard_cap is 1000 and is accepted",
            hard_cap: 1000,
            expect_err_substr: None,
        },
        Case {
            name: "hard_cap 9 is too small",
            hard_cap: 9,
            expect_err_substr: Some("hard_cap"),
        },
        Case {
            name: "hard_cap 100_001 is too large",
            hard_cap: 100_001,
            expect_err_substr: Some("hard_cap"),
        },
    ];

    for case in &cases {
        let mut cfg = EdgionControllerConfig::default();
        cfg.metrics.resource_abnormal.hard_cap = case.hard_cap;
        let result = cfg.validate_metrics();
        match case.expect_err_substr {
            None => {
                assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
            }
            Some(substr) => {
                assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                let msg = format!("{:?}", result.unwrap_err());
                assert!(
                    msg.contains(substr),
                    "case '{}': error {:?} does not contain {:?}",
                    case.name,
                    msg,
                    substr
                );
            }
        }
    }
}

// ── Group I: cli_tokens parse + validate ─────────────────────────────────

#[test]
fn cli_tokens_parse_and_validate() {
    let cfg: EdgionControllerConfig = serde_yaml::from_str(
            "cli_tokens:\n  - name: bot\n    token_sha256: \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n    rbac:\n      allow:\n        - verbs: [get, list]\n          kinds: [EdgionConfigData]\n",
        )
        .unwrap();
    assert_eq!(cfg.cli_tokens.len(), 1);
    assert_eq!(cfg.cli_tokens[0].name, "bot");
    assert!(cfg.validate_cli_tokens().is_ok());
}

#[test]
fn cli_tokens_validate_rejects_bad_hash_dup_name_dup_hash() {
    let bad_hex = CliTokenConfig {
        name: "a".into(),
        token_sha256: "xyz".into(),
        rbac: FedRbacConfig::default(),
    };
    assert!(EdgionControllerConfig {
        cli_tokens: vec![bad_hex],
        ..Default::default()
    }
    .validate_cli_tokens()
    .is_err());

    let h = "0".repeat(64);
    let dup_name = vec![
        CliTokenConfig {
            name: "a".into(),
            token_sha256: h.clone(),
            rbac: FedRbacConfig::default(),
        },
        CliTokenConfig {
            name: "a".into(),
            token_sha256: "1".repeat(64),
            rbac: FedRbacConfig::default(),
        },
    ];
    assert!(EdgionControllerConfig {
        cli_tokens: dup_name,
        ..Default::default()
    }
    .validate_cli_tokens()
    .is_err());

    let dup_hash = vec![
        CliTokenConfig {
            name: "a".into(),
            token_sha256: h.clone(),
            rbac: FedRbacConfig::default(),
        },
        CliTokenConfig {
            name: "b".into(),
            token_sha256: h,
            rbac: FedRbacConfig::default(),
        },
    ];
    assert!(EdgionControllerConfig {
        cli_tokens: dup_hash,
        ..Default::default()
    }
    .validate_cli_tokens()
    .is_err());
}

// ── Group H: CenterClientConfig enabled + rbac ───────────────────────────

#[test]
fn center_enabled_defaults_to_true() {
    let cfg: CenterClientConfig = serde_yaml::from_str("address: \"https://c:12251\"\nname: n\ncluster: c\n").unwrap();
    assert!(cfg.enabled, "enabled must default to true (backward compatible)");
    assert!(cfg.rbac.is_none(), "absent rbac => None => deny-all downstream");
}

#[test]
fn center_rbac_parses_allow_rules() {
    let cfg: CenterClientConfig = serde_yaml::from_str(
        r#"
address: a
name: n
cluster: c
enabled: false
rbac:
  allow:
    - verbs: [get, list]
      kinds: [EdgionConfigData]
    - verbs: [reload]
      kinds: ["*"]
"#,
    )
    .unwrap();
    assert!(!cfg.enabled);
    let rbac = cfg.rbac.expect("rbac present");
    assert_eq!(rbac.allow.len(), 2);
    assert_eq!(rbac.allow[0].verbs, vec!["get", "list"]);
    assert_eq!(rbac.allow[1].kinds, vec!["*"]);
}

#[test]
fn center_rbac_validate_rejects_empty_rule() {
    let rbac = FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec![],
            kinds: vec!["EdgionConfigData".into()],
        }],
    };
    assert!(rbac.validate().is_err(), "a rule with no verbs is malformed");
}

// ── Group J: listen address defaults ────────────────────────────────────

#[test]
fn test_listen_address_defaults() {
    let cfg = EdgionControllerConfig::default();
    assert_eq!(cfg.grpc_listen(), "0.0.0.0:12151");
    assert_eq!(cfg.admin_listen(), "127.0.0.1:12101");
    assert_eq!(cfg.probe_listen(), "0.0.0.0:12100");
    assert_eq!(cfg.metrics_listen(), "0.0.0.0:12190");
}
