use super::*;
use edgion_resources::rate_limit_local_config::{
    get_gateway_instance_count, set_gateway_instance_count, DEFAULT_ESTIMATOR_SLOTS_K, DEFAULT_MAX_ESTIMATOR_SLOTS_K,
};

fn unknown_paths(yaml: &str) -> UnknownConfigFields {
    deserialize_process_config(yaml).unwrap().1
}

#[test]
fn clean_process_config_has_no_unknown_fields() {
    let yaml = r#"
gateway:
  server_addrs: ["http://127.0.0.1:12151"]
metrics:
  request_duration:
    enabled: true
    idle_timeout_secs: 30
  backend_requests:
    labels:
      gateway: true
access_log:
  enabled: true
  output:
    localFile:
      path: logs/access.log
      queueSize: 1024
      rotation:
        strategy: daily
        maxFiles: 7
  fallback:
    localFile:
      path: logs/fallback.log
conf_sync_security:
  active: insecure
  certs:
    - name: insecure
      skip_tls: true
  rotation:
    policy: lifetime
lru_cache:
  default_capacity: 128
  default_shards: 2
"#;

    let (config, unknown) = deserialize_process_config(yaml).unwrap();
    assert!(unknown.paths().next().is_none());
    assert!(!unknown.is_truncated());
    assert_eq!(config.lru_cache.default_capacity, 128);
    assert_eq!(config.lru_cache.default_shards, 2);
}

#[test]
fn shipped_gateway_process_config_has_no_unknown_fields() {
    let (_, unknown) = deserialize_process_config(include_str!("../../../../config/edgion-gateway.yaml")).unwrap();
    assert_eq!(unknown.paths().collect::<Vec<_>>(), Vec::<&str>::new());
    assert!(!unknown.is_truncated());
}

#[test]
fn process_config_collects_root_and_nested_unknown_field_paths() {
    let yaml = r#"
geoip:
  db_path: /var/lib/edgion/geo.mmdb
metrics:
  request_duration:
    idle_timeout_sec: 30
access_log:
  output:
    localFile:
      queue_size: 1024
      rotation:
        max_files: 7
  fallback:
    localFile:
      fallback_typo: true
conf_sync_security:
  certs:
    - name: insecure
      skip_tls: true
      cert_typo: ignored
  rotation:
    lifetime_ratio_typo: 0.4
future_section:
  nested_secret_value: must-not-be-reported
"#;

    let (config, unknown) = deserialize_process_config(yaml).unwrap();
    assert_eq!(config.metrics.request_duration.idle_timeout_secs, 600);
    assert!(matches!(
        config.access_log.output,
        edgion_resources::StringOutput::LocalFile(_)
    ));
    assert_eq!(
        unknown.paths().collect::<Vec<_>>(),
        vec![
            "access_log.fallback.localFile.fallback_typo",
            "access_log.output.localFile.queue_size",
            "access_log.output.localFile.rotation.max_files",
            "conf_sync_security.certs[0].cert_typo",
            "conf_sync_security.rotation.lifetime_ratio_typo",
            "future_section",
            "geoip",
            "metrics.request_duration.idle_timeout_sec",
        ]
    );
    assert!(!unknown.is_truncated());
}

#[test]
fn process_config_unknown_field_diagnostics_are_bounded() {
    let yaml = (0..MAX_UNKNOWN_CONFIG_FIELDS + 5)
        .map(|index| format!("unknown_{index:02}: true\n"))
        .collect::<String>();
    let unknown = unknown_paths(&yaml);
    assert_eq!(unknown.paths().count(), MAX_UNKNOWN_CONFIG_FIELDS);
    assert!(unknown.is_truncated());
}

#[test]
fn process_config_keeps_strict_substructures_strict() {
    for yaml in [
        "request_mirror:\n  max_concurent: 12\n",
        "access_log_options:\n  include_raw_uri: true\n",
        "path_normalization:\n  reject_escape_slashes: true\n",
        "gateway:\n  geoip:\n    dbPath: /tmp/geo.mmdb\n",
        "access_log:\n  output:\n    unknownSink: {}\n",
    ] {
        assert!(deserialize_process_config(yaml).is_err(), "must remain strict: {yaml}");
    }
}

#[test]
fn process_config_without_a_file_has_no_unknown_diagnostics() {
    let loaded = EdgionGatewayConfig::load(EdgionGatewayConfig::default()).unwrap();
    assert!(loaded.unknown_fields.paths().next().is_none());
    assert!(!loaded.unknown_fields.is_truncated());
}

#[test]
fn process_config_load_preserves_cli_precedence_and_unknown_diagnostics() {
    use std::io::Write;

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"gateway:\n  admin_listen: 127.0.0.1:12002\n  bind_ip: 127.0.0.1\nroot_typo: ignored\n")
        .unwrap();
    let cli = EdgionGatewayConfig {
        config_file: Some(file.path().to_string_lossy().into_owned()),
        gateway: GatewayConfig {
            admin_listen: Some("127.0.0.1:12003".to_string()),
            bind_ip: Some("::1".parse().unwrap()),
            ..Default::default()
        },
        ..Default::default()
    };

    let loaded = EdgionGatewayConfig::load(cli).unwrap();
    assert_eq!(loaded.config.gateway.admin_listen.as_deref(), Some("127.0.0.1:12003"));
    assert_eq!(loaded.config.gateway.bind_ip, Some("::1".parse().unwrap()));
    assert_eq!(loaded.unknown_fields.paths().collect::<Vec<_>>(), vec!["root_typo"]);
}

/// The gateway instance count is a PROCESS-GLOBAL, and `cargo test` runs the tests in
/// this binary on parallel threads. Without serialization, `set_and_get` writing 5 can
/// land between `never_zero`'s write of 0 and its read, so `never_zero` observes 5 and
/// fails an assertion about clamping that has nothing to do with what actually broke.
/// (Observed exactly once under a full `cargo test --all`, where cross-binary CPU
/// contention widens the window; the three tests pass on their own.)
///
/// `parking_lot::Mutex` per the workspace lint: it does not poison, so a sibling test
/// panicking while holding the lock cannot cascade into unrelated failures here.
static INSTANCE_COUNT_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn test_gateway_instance_count_default() {
    let _guard = INSTANCE_COUNT_LOCK.lock();
    // Default should be at least 1
    assert!(get_gateway_instance_count() >= 1);
}

#[test]
fn test_gateway_instance_count_set_and_get() {
    let _guard = INSTANCE_COUNT_LOCK.lock();
    set_gateway_instance_count(5);
    assert_eq!(get_gateway_instance_count(), 5);
    set_gateway_instance_count(1); // cleanup
}

#[test]
fn test_gateway_instance_count_never_zero() {
    let _guard = INSTANCE_COUNT_LOCK.lock();
    set_gateway_instance_count(0); // Try to set zero
    assert_eq!(get_gateway_instance_count(), 1); // Should be clamped to 1
    set_gateway_instance_count(1); // cleanup: leave the global in its default state
}

#[test]
fn test_tiny_ufo_ttl_cache_global_config_default() {
    let config = TinyUfoTtlCacheGlobalConfig::default();
    assert_eq!(config.default_max_entries, 1024);
    assert_eq!(config.default_max_logical_bytes, "4MiB");
}

#[test]
fn test_tiny_ufo_ttl_cache_default_limits_resolve() {
    let limits = resolve_tiny_ufo_ttl_cache_limits_with(
        &TinyUfoTtlCacheGlobalConfig::default(),
        &TinyUfoTtlCacheConfig::default(),
    )
    .expect("default limits must be valid");
    assert_eq!(limits.max_entries.get(), 1024);
    assert_eq!(limits.max_logical_bytes.get(), 4 * 1024 * 1024);
}

#[test]
fn test_lru_cache_global_config_remains_available() {
    let config = LruCacheGlobalConfig::default();
    assert_eq!(config.default_capacity, 1024);
    assert_eq!(config.default_shards, 0);
    assert!(get_default_lru_cache_capacity() > 0);
}

#[test]
fn lru_cache_limits_reject_zero_oversized_and_capacity_expanding_shards() {
    assert!(validate_effective_lru_cache_limits(0, 1).is_err());
    assert!(validate_effective_lru_cache_limits(MAX_LRU_CACHE_CAPACITY + 1, 1).is_err());
    assert!(validate_effective_lru_cache_limits(4, 5).is_err());
    assert!(validate_effective_lru_cache_limits(1024, MAX_LRU_CACHE_SHARDS + 1).is_err());
    assert!(validate_effective_lru_cache_limits(1024, 8).is_ok());
}

#[test]
fn test_rate_limit_local_global_config_default() {
    let config = RateLimitLocalGlobalConfig::default();
    assert_eq!(config.default_estimator_slots_k, DEFAULT_ESTIMATOR_SLOTS_K);
    assert_eq!(config.max_estimator_slots_k, DEFAULT_MAX_ESTIMATOR_SLOTS_K);
    assert_eq!(config.gateway_instance_count, 1);
}

#[test]
fn test_admin_listen_defaults_to_loopback() {
    // Security default: when admin_listen is unset, the admin API must bind
    // loopback so it is not exposed cluster-wide. Reverting this to 0.0.0.0
    // would reopen the unauthenticated admin-surface exposure (harden Task 2).
    let cfg = EdgionGatewayConfig::default();
    assert_eq!(cfg.admin_listen(), None);
    assert_eq!(cfg.admin_listen_or_default(), "127.0.0.1:12001");
}

#[test]
fn request_mirror_process_limits_survive_cli_merge() {
    let yaml = "request_mirror:\n  max_concurrent: 17\n  max_buffered_bytes: 8MiB\n";
    let de = serde_yaml::Deserializer::from_str(yaml);
    let mut file: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
    EdgionGatewayConfig::merge(&mut file, &EdgionGatewayConfig::default());
    let resolved = file.request_mirror.resolve().unwrap();
    assert_eq!(resolved.max_concurrent, 17);
    assert_eq!(resolved.max_buffered_bytes, 8 * 1024 * 1024);
    assert!(serde_yaml::from_str::<EdgionGatewayConfig>("request_mirror:\n  max_concurent: 17\n").is_err());
}

#[test]
fn access_log_options_use_the_canonical_key_in_root_gateway_yaml() {
    let canonical: EdgionGatewayConfig = serde_yaml::from_str("access_log_options:\n  includeRawUri: true\n").unwrap();
    assert!(canonical.access_log_options.include_raw_uri);

    assert!(serde_yaml::from_str::<EdgionGatewayConfig>("access_log_options:\n  include_raw_uri: true\n").is_err());
}

#[test]
fn test_admin_listen_explicit_value_is_respected() {
    // Operators opting into cluster exposure (e.g. pod-IP kubelet probes)
    // set admin_listen explicitly; the default must not override it.
    let mut cfg = EdgionGatewayConfig::default();
    cfg.gateway.admin_listen = Some("0.0.0.0:5810".to_string());
    assert_eq!(cfg.admin_listen_or_default(), "0.0.0.0:5810");
}

#[test]
fn test_probe_listen_defaults_to_outward() {
    // The probe port carries only non-sensitive liveness/readiness signals and
    // binds 0.0.0.0 by default so a kubelet httpGet can reach it over the pod IP.
    let cfg = EdgionGatewayConfig::default();
    assert_eq!(cfg.probe_listen(), None);
    assert_eq!(cfg.probe_listen_or_default(), "0.0.0.0:12000");
}

#[test]
fn test_probe_listen_explicit_value_is_respected() {
    let mut cfg = EdgionGatewayConfig::default();
    cfg.gateway.probe_listen = Some("127.0.0.1:28000".to_string());
    assert_eq!(cfg.probe_listen_or_default(), "127.0.0.1:28000");
}

#[test]
fn test_probe_listen_cli_survives_config_file_merge() {
    // Guards the merge() wiring: without the merge branch, a CLI --probe-listen
    // is silently dropped whenever a config file is loaded.
    let mut base = EdgionGatewayConfig::default();
    let mut cli = EdgionGatewayConfig::default();
    cli.gateway.probe_listen = Some("0.0.0.0:39999".to_string());
    EdgionGatewayConfig::merge(&mut base, &cli);
    assert_eq!(base.gateway.probe_listen.as_deref(), Some("0.0.0.0:39999"));
}

#[test]
fn server_addrs_merge_and_default() {
    let mut base = EdgionGatewayConfig::default();
    base.gateway.server_addrs = vec!["10.0.0.1:9090".into()];
    let mut cli = EdgionGatewayConfig::default(); // empty server_addrs
                                                  // An empty CLI vec must NOT clobber the file value (spec §7.1).
    EdgionGatewayConfig::merge(&mut base, &cli);
    assert_eq!(base.server_addrs(), vec!["10.0.0.1:9090".to_string()]);
    // A non-empty CLI vec overrides.
    cli.gateway.server_addrs = vec!["10.0.0.2:9090".into(), "10.0.0.3:9090".into()];
    EdgionGatewayConfig::merge(&mut base, &cli);
    assert_eq!(base.server_addrs().len(), 2);
}

#[test]
fn test_access_log_sensitive_default_is_replace() {
    // Privacy-by-default: `ext` rendering (and the built-in sensitive
    // fields) follow this mode, so shipping `off` would leave the
    // EdgionGatewayConfig whitelist inert out of the box.
    let cfg: EdgionGatewayConfig = serde_yaml::from_str("{}").unwrap();
    assert!(matches!(
        cfg.access_log_sensitive.mode,
        edgion_resources::sensitive::SensitiveMode::Replace
    ));
}

#[test]
fn test_access_log_sensitive_replace_mode() {
    let yaml_str = r#"
access_log_sensitive:
  mode: replace
  replace_with: "{MSKED}"
"#;
    let cfg: EdgionGatewayConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(matches!(
        cfg.access_log_sensitive.mode,
        edgion_resources::sensitive::SensitiveMode::Replace
    ));
    assert_eq!(cfg.access_log_sensitive.replace_with, "{MSKED}");
}

#[test]
fn test_access_log_sensitive_wrap_mode() {
    let yaml_str = r#"
access_log_sensitive:
  mode: wrap
  wrap_prefix: "<E>"
  wrap_suffix: "</E>"
"#;
    let cfg: EdgionGatewayConfig = serde_yaml::from_str(yaml_str).unwrap();
    assert!(matches!(
        cfg.access_log_sensitive.mode,
        edgion_resources::sensitive::SensitiveMode::Wrap
    ));
    assert_eq!(cfg.access_log_sensitive.wrap_prefix, "<E>");
}

#[test]
fn test_metrics_listen_defaults_to_management_block() {
    // The metrics listener default must use the Gateway management port block
    // (120xx). Changing this would silently break the port-unification plan.
    let cfg = MetricsConfig::default();
    assert_eq!(cfg.listen, "0.0.0.0:12090");
}

#[test]
fn genai_token_metric_is_disabled_by_default_and_yaml_can_enable_it() {
    assert!(!MetricsConfig::default().genai_tokens.enabled);

    let cfg: MetricsConfig = serde_yaml::from_str(
        r#"
genai_tokens:
  enabled: true
"#,
    )
    .unwrap();
    assert!(cfg.genai_tokens.enabled);
}

#[test]
fn integration_testing_mode_env_accepts_truthy_values() {
    for v in ["1", "true", "TRUE", "Yes", "on", "  on  "] {
        assert!(
            integration_testing_mode_env_enabled(Some(v)),
            "expected {v:?} to enable integration testing mode"
        );
    }
}

#[test]
fn integration_testing_mode_env_rejects_falsey_and_unset() {
    for v in ["0", "false", "no", "off", "", "   ", "enable", "2"] {
        assert!(
            !integration_testing_mode_env_enabled(Some(v)),
            "expected {v:?} NOT to enable integration testing mode"
        );
    }
    assert!(
        !integration_testing_mode_env_enabled(None),
        "unset env must not enable integration testing mode"
    );
}

#[test]
fn resolve_admin_token_prefers_nonempty_env() {
    assert_eq!(
        resolve_admin_token(Some("env-token".into()), Some("config-token")),
        Some("env-token".to_string())
    );
}

#[test]
fn resolve_admin_token_empty_env_falls_through_to_config() {
    // present-but-empty env must not shadow a real config value
    assert_eq!(
        resolve_admin_token(Some("   ".into()), Some("config-token")),
        Some("config-token".to_string())
    );
    assert_eq!(
        resolve_admin_token(Some("".into()), Some("config-token")),
        Some("config-token".to_string())
    );
}

#[test]
fn resolve_admin_token_trims_and_treats_empty_as_none() {
    assert_eq!(resolve_admin_token(None, None), None);
    assert_eq!(resolve_admin_token(None, Some("  ")), None);
    assert_eq!(
        resolve_admin_token(Some("  spaced  ".into()), None),
        Some("spaced".to_string())
    );
}

#[test]
fn admin_token_is_not_serialized() {
    let cfg = EdgionGatewayConfig {
        gateway: GatewayConfig {
            admin_token: Some("must-not-leak".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let serialized = serde_yaml::to_string(&cfg).unwrap();
    assert!(
        !serialized.contains("must-not-leak"),
        "admin_token must not serialize out"
    );
}

#[test]
fn admin_token_is_redacted_in_debug() {
    let cfg = GatewayConfig {
        admin_token: Some("must-not-leak".into()),
        ..Default::default()
    };
    let dbg = format!("{:?}", cfg);
    assert!(!dbg.contains("must-not-leak"), "admin_token must be redacted in Debug");
    assert!(dbg.contains("[redacted]"));
}

/// `GatewayConfig` derives `Debug` (see the struct doc): this is now a
/// smoke test that every field, including a nested config struct like
/// `geoip`, actually reaches the derived output — not a guard against a
/// hand-maintained `.field(...)` list going stale, which no longer exists
/// for this struct.
#[test]
fn debug_output_includes_geoip() {
    let cfg = GatewayConfig::default();
    let dbg = format!("{:?}", cfg);
    assert!(
        dbg.contains("geoip"),
        "GatewayConfig Debug output must include the geoip field"
    );
}

#[test]
fn require_fips_env_truthy_values() {
    assert!(require_fips_env_enabled(Some("1")));
    assert!(require_fips_env_enabled(Some(" TRUE ")));
    assert!(require_fips_env_enabled(Some("yes")));
    assert!(!require_fips_env_enabled(Some("0")));
    assert!(!require_fips_env_enabled(Some("")));
    assert!(!require_fips_env_enabled(None));
}

#[test]
fn outbound_egress_defaults_are_permissive_and_short_lived() {
    let cfg = GatewayConfig::default();
    assert!(
        !cfg.outbound_egress_block_private,
        "private ranges must be allowed by default, matching classify_backend_addr"
    );
    assert_eq!(cfg.outbound_egress_dns_cache_ttl_ms, 5_000);
    assert_eq!(cfg.outbound_http2_max_header_list_size, 16 * 1024);
}

#[test]
fn outbound_egress_parses_from_yaml() {
    let cfg: GatewayConfig = serde_yaml::from_str(
        "outbound_egress_block_private: true\noutbound_egress_dns_cache_ttl_ms: 30000\n\
             outbound_http2_max_header_list_size: 65536\n",
    )
    .unwrap();
    assert!(cfg.outbound_egress_block_private);
    assert_eq!(cfg.outbound_egress_dns_cache_ttl_ms, 30_000);
    assert_eq!(cfg.outbound_http2_max_header_list_size, 64 * 1024);
}

#[test]
fn wasm_oci_cache_fields_parse_and_default() {
    let cfg = GatewayConfig::default();
    assert_eq!(cfg.wasm_oci_cache_ttl_secs, 86_400);
    assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 60);
    assert_eq!(cfg.wasm_oci_cache_max_bytes, 256 * 1024 * 1024);
    assert_eq!(cfg.wasm_oci_cache_max_entries, 256);
    let cfg: GatewayConfig = serde_yaml::from_str(
        "wasm_oci_cache_max_entries: 0\nwasm_oci_cache_ttl_secs: 60\n\
             wasm_oci_cache_purge_interval_secs: 30\nwasm_oci_cache_max_bytes: 0\n",
    )
    .unwrap();
    assert_eq!(cfg.wasm_oci_cache_max_entries, 0);
    assert_eq!(cfg.wasm_oci_cache_ttl_secs, 60);
    assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 30);
    assert_eq!(cfg.wasm_oci_cache_max_bytes, 0);
    // A zero purge interval parses as-is (no "disable" semantics); the
    // busy-loop guard is a startup clamp in `configure_oci_memory_cache`,
    // not a parse-time rejection.
    let cfg: GatewayConfig = serde_yaml::from_str("wasm_oci_cache_purge_interval_secs: 0\n").unwrap();
    assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 0);
    // Pull tuning defaults + YAML override.
    assert_eq!(GatewayConfig::default().wasm_oci_pull_timeout_secs, 60);
    assert_eq!(GatewayConfig::default().wasm_oci_pull_attempts, 3);
    let cfg: GatewayConfig =
        serde_yaml::from_str("wasm_oci_pull_timeout_secs: 15\nwasm_oci_pull_attempts: 5\n").unwrap();
    assert_eq!(cfg.wasm_oci_pull_timeout_secs, 15);
    assert_eq!(cfg.wasm_oci_pull_attempts, 5);
}

#[test]
fn outbound_egress_fields_come_from_config_file_not_cli() {
    // Both fields are `#[arg(skip)]`: clap never populates them, and
    // `merge()` does not copy them from the CLI-parsed struct into the
    // file-loaded base config. Their only source is the config file (or
    // the corresponding env var, applied separately in cli/mod.rs). This
    // guards against a regression that re-adds a clap flag for these
    // fields without also wiring `merge()` to honor it.
    let yaml = "gateway:\n  outbound_egress_block_private: true\n  outbound_egress_dns_cache_ttl_ms: 30000\n";
    let mut base: EdgionGatewayConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(base.gateway.outbound_egress_block_private);
    assert_eq!(base.gateway.outbound_egress_dns_cache_ttl_ms, 30_000);

    // Simulate a CLI-parsed struct carrying different values (as would
    // happen if a clap flag were mistakenly reintroduced and passed).
    let mut cli = EdgionGatewayConfig::default();
    cli.gateway.outbound_egress_block_private = false;
    cli.gateway.outbound_egress_dns_cache_ttl_ms = 5_000;

    EdgionGatewayConfig::merge(&mut base, &cli);

    // merge() must leave these fields alone: the config-file values survive.
    assert!(base.gateway.outbound_egress_block_private);
    assert_eq!(base.gateway.outbound_egress_dns_cache_ttl_ms, 30_000);

    // And GatewayConfig::default() still gives the documented defaults.
    assert!(!GatewayConfig::default().outbound_egress_block_private);
    assert_eq!(GatewayConfig::default().outbound_egress_dns_cache_ttl_ms, 5_000);
}

#[test]
fn outbound_block_private_env_truthy_values() {
    assert!(outbound_block_private_env_enabled(Some("1")));
    assert!(outbound_block_private_env_enabled(Some(" TRUE ")));
    assert!(outbound_block_private_env_enabled(Some("yes")));
    assert!(outbound_block_private_env_enabled(Some("On")));
    assert!(!outbound_block_private_env_enabled(Some("0")));
    assert!(!outbound_block_private_env_enabled(Some("")));
    assert!(!outbound_block_private_env_enabled(None));
}

#[test]
fn outbound_egress_dns_cache_ttl_ms_env_overrides_when_valid() {
    assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some("30000")), 30_000);
    assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some(" 12 ")), 12);
}

#[test]
fn outbound_egress_dns_cache_ttl_ms_falls_back_on_unparseable_env() {
    // An unparseable TTL must not panic or fail startup; the configured
    // value is kept and a WARN is logged.
    assert_eq!(
        resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some("not-a-number")),
        5_000
    );
    assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(7_500, Some("")), 7_500);
    assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(9_000, None), 9_000);
}

#[test]
fn check_fips_requirement_ok_when_not_required() {
    assert!(check_fips_requirement(false, false).is_ok());
    assert!(check_fips_requirement(false, true).is_ok());
}

#[test]
fn check_fips_requirement_ok_when_required_and_present() {
    assert!(check_fips_requirement(true, true).is_ok());
}

#[test]
fn check_fips_requirement_err_when_required_but_missing() {
    let err = check_fips_requirement(true, false).unwrap_err();
    assert!(err.contains("FIPS"));
}

#[test]
fn otlp_reference_uses_production_yaml_deserialization() {
    let yaml = "access_log:\n  output:\n    otlp:\n      linkSysRef:\n        namespace: observability\n        name: collector\n";
    let de = serde_yaml::Deserializer::from_str(yaml);
    let cfg: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
    let edgion_resources::StringOutput::Otlp { link_sys_ref } = cfg.access_log.output else {
        panic!("expected OTLP reference output");
    };
    assert_eq!(link_sys_ref.key().unwrap(), "observability/collector");
}

#[test]
fn otlp_invalid_references_fail_every_process_log_output_before_startup() {
    // Disabled logs and fallback positions are still configuration inputs;
    // validation must precede logger construction and identity emission.
    for log in ["access_log", "ssl_log", "tcp_log", "tls_log", "udp_log"] {
        for position in ["output", "fallback"] {
            for field in ["namespace", "name"] {
                for invalid in ["".to_owned(), "private-sentinel/invalid".to_owned(), "x".repeat(254)] {
                    let mut reference = serde_json::json!({"namespace": "default", "name": "collector"});
                    reference[field] = serde_json::Value::String(invalid.clone());
                    let mut config = serde_json::json!({});
                    config[log] = serde_json::json!({"enabled": false});
                    config[log][position] = serde_json::json!({"otlp": {"linkSysRef": reference}});
                    let yaml = serde_yaml::to_string(&config).unwrap();
                    let de = serde_yaml::Deserializer::from_str(&yaml);
                    let result: Result<EdgionGatewayConfig, _> =
                        serde_yaml::with::singleton_map_recursive::deserialize(de);
                    let error = result
                        .expect_err("invalid OTLP reference must fail config loading")
                        .to_string();
                    if !invalid.is_empty() {
                        assert!(
                            !error.contains(&invalid),
                            "reference was echoed: {log}.{position}.{field}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn otlp_inline_process_configuration_is_rejected() {
    for inline in [
        "endpoint: https://private-sentinel.example:4317",
        "protocol: grpc",
        "headers: {authorization: private-sentinel}",
    ] {
        let yaml = format!(
                "access_log:\n  output:\n    otlp:\n      linkSysRef: {{namespace: default, name: collector}}\n      {inline}\n"
            );
        let de = serde_yaml::Deserializer::from_str(&yaml);
        let result: Result<EdgionGatewayConfig, _> = serde_yaml::with::singleton_map_recursive::deserialize(de);
        let error = result.expect_err("inline OTLP fields must be rejected").to_string();
        assert!(!error.contains("private-sentinel"));
    }
}

#[test]
fn fips_policy_env_mode_parses_tristate() {
    use crate::tls::fips_profile::FipsPolicyMode;

    assert!(matches!(
        fips_policy_env_mode(Some("auto")),
        EnvPolicy::Set(FipsPolicyMode::Auto)
    ));
    assert!(matches!(
        fips_policy_env_mode(Some(" On ")),
        EnvPolicy::Set(FipsPolicyMode::On)
    ));
    assert!(matches!(
        fips_policy_env_mode(Some("off")),
        EnvPolicy::Set(FipsPolicyMode::Off)
    ));
    // No compatibility layer: the legacy truthy/falsey vocabulary of the old
    // boolean flag is no longer recognized.
    assert!(matches!(fips_policy_env_mode(Some("1")), EnvPolicy::Invalid(_)));
    assert!(matches!(fips_policy_env_mode(Some("false")), EnvPolicy::Invalid(_)));
}

#[test]
fn fips_policy_env_mode_unset_and_empty_fall_through() {
    assert!(matches!(fips_policy_env_mode(None), EnvPolicy::Unset));
    assert!(matches!(fips_policy_env_mode(Some("")), EnvPolicy::Unset));
    assert!(matches!(fips_policy_env_mode(Some("   ")), EnvPolicy::Unset));
}

#[test]
fn fips_policy_env_mode_reports_garbage_instead_of_guessing() {
    // A typo must not silently change policy in either direction; the caller
    // WARNs and falls through to the config file.
    match fips_policy_env_mode(Some("enabled")) {
        EnvPolicy::Invalid(raw) => assert_eq!(raw, "enabled"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn merge_propagates_fips_policy_from_cli_and_preserves_file_when_omitted() {
    use crate::tls::fips_profile::FipsPolicyMode;

    // Regression test for the pre-existing bug: --fips-policy was parsed and
    // then discarded by merge(), so the flag did nothing.
    let mut file = EdgionGatewayConfig::default();
    file.gateway.fips_policy = Some(FipsPolicyMode::On);

    let mut cli = EdgionGatewayConfig::default();
    cli.gateway.fips_policy = Some(FipsPolicyMode::Off);
    EdgionGatewayConfig::merge(&mut file, &cli);
    assert_eq!(
        file.gateway.fips_policy,
        Some(FipsPolicyMode::Off),
        "CLI must override the file"
    );

    // Omitted on the CLI (None) must NOT clobber the file's value. This is
    // why the field is Option: a bare enum with a clap default_value would
    // make "omitted" indistinguishable from "--fips-policy=auto".
    let mut file = EdgionGatewayConfig::default();
    file.gateway.fips_policy = Some(FipsPolicyMode::On);
    let cli = EdgionGatewayConfig::default();
    EdgionGatewayConfig::merge(&mut file, &cli);
    assert_eq!(
        file.gateway.fips_policy,
        Some(FipsPolicyMode::On),
        "omitted CLI must preserve the file"
    );
}

#[test]
fn merge_propagates_require_fips_with_or_semantics() {
    let mut file = EdgionGatewayConfig::default();
    let mut cli = EdgionGatewayConfig::default();
    cli.gateway.require_fips = true;
    EdgionGatewayConfig::merge(&mut file, &cli);
    assert!(
        file.gateway.require_fips,
        "CLI --require-fips must reach the merged config"
    );

    // OR semantics: a file `true` survives an absent CLI flag.
    let mut file = EdgionGatewayConfig::default();
    file.gateway.require_fips = true;
    let cli = EdgionGatewayConfig::default();
    EdgionGatewayConfig::merge(&mut file, &cli);
    assert!(file.gateway.require_fips);
}

#[test]
fn gateway_config_deserializes_fips_policy() {
    use crate::tls::fips_profile::FipsPolicyMode;

    let yaml = "fips_policy: auto\n";
    let parsed: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(parsed.fips_policy, Some(FipsPolicyMode::Auto));

    // Bare `on` resolves to a YAML boolean, so it must be quoted to reach
    // the string parser.
    let yaml = "fips_policy: \"on\"\n";
    let parsed: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(parsed.fips_policy, Some(FipsPolicyMode::On));

    // Omitted => None => the startup path applies Auto.
    let parsed: GatewayConfig = serde_yaml::from_str("{}").unwrap();
    assert_eq!(parsed.fips_policy, None);

    // No compatibility layer: a YAML boolean is rejected outright.
    let yaml = "fips_policy: true\n";
    assert!(serde_yaml::from_str::<GatewayConfig>(yaml).is_err());
}

#[test]
fn response_cache_restart_limits_include_standing_epoch_and_reject_partial_population_budget() {
    let limits = ResponseCacheLimits::default();
    let infrastructure = limits.validate().unwrap();
    assert_eq!(
        infrastructure,
        limits.max_entries * 4096 + limits.shards * 4096 + 4096 + crate::response_cache::epoch_control_bytes()
    );
    let mut reduced = ResponseCacheLimits {
        max_entries: 2,
        shards: 1,
        max_body_bytes: 64,
        max_metadata_bytes: 256,
        total_bytes: 65536,
        pending_bytes: 16384,
    };
    let infrastructure = reduced.validate().unwrap();
    let exact = infrastructure
        + reduced.max_entries * (reduced.max_body_bytes + reduced.max_metadata_bytes + 1024)
        + reduced.pending_bytes;
    reduced.total_bytes = exact;
    assert!(reduced.validate().is_ok());
    reduced.total_bytes -= 1;
    assert!(reduced.validate().is_err());
    reduced.total_bytes = 65536;
    reduced.shards = 3;
    assert!(reduced.validate().is_err());
    reduced.shards = 1;
    reduced.max_body_bytes = 0;
    assert!(reduced.validate().is_err());
    reduced.max_body_bytes = 64;
    reduced.pending_bytes = usize::MAX;
    assert!(reduced.validate().is_err());
    assert!(serde_yaml::from_str::<EdgionGatewayConfig>("response_cache:\n  unknown: 1\n").is_err());
}

#[test]
fn wasm_guest_memory_budget_is_restart_only_and_parses() {
    let cfg: GatewayConfig = serde_yaml::from_str("wasm_guest_memory_bytes: 1048576\n").unwrap();
    assert_eq!(cfg.wasm_guest_memory_bytes, 1048576);
    assert_eq!(GatewayConfig::default().wasm_guest_memory_bytes, 512 * 1024 * 1024);
    assert!(serde_yaml::from_str::<GatewayConfig>("wasm_guest_memory_bytes: -1").is_err());
}
