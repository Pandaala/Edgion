use super::*;
use crate::cli::EdgionGatewayCli;
use clap::Parser;
use std::io::Write;
use tempfile::NamedTempFile;

fn load_server_conf(yaml: Option<&str>, args: &[&str]) -> (EdgionGatewayConfig, ServerConf) {
    let file = yaml.map(|content| {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    });
    let mut argv = vec!["edgion-gateway"];
    if let Some(file) = &file {
        argv.extend(["--config-file", file.path().to_str().unwrap()]);
    }
    argv.extend_from_slice(args);
    let mut cli = EdgionGatewayCli::try_parse_from(argv).unwrap();
    if file.is_none() {
        // Clap supplies a default file path; exercise load(None) without changing server options.
        cli.config.config_file = None;
    }
    let config = EdgionGatewayConfig::load(cli.config).unwrap().config;
    let conf = create_server_conf(&config).unwrap();
    (config, conf)
}

const PROCESS_SERVER_YAML: &str = r#"
server:
  threads: 4
  work_stealing: true
  grace_period_seconds: 40
  graceful_shutdown_timeout_seconds: 20
  upstream_keepalive_pool_size: 256
  error_log: logs/file-error.log
"#;

#[test]
fn process_cli_overrides_reach_server_conf_with_and_without_yaml() {
    for yaml in [Some(PROCESS_SERVER_YAML), None] {
        let (_, conf) = load_server_conf(
            yaml,
            &[
                "--threads",
                "1",
                "--work-stealing",
                "false",
                "--grace-period",
                "3",
                "--graceful-shutdown-timeout",
                "2",
                "--upstream-keepalive-pool-size",
                "7",
                "--error-log",
                "logs/cli-error.log",
            ],
        );
        assert_eq!(conf.threads, 1);
        assert!(!conf.work_stealing);
        assert_eq!(conf.grace_period_seconds, Some(3));
        assert_eq!(conf.graceful_shutdown_timeout_seconds, Some(2));
        assert_eq!(conf.upstream_keepalive_pool_size, 7);
        assert_eq!(conf.error_log.as_deref(), Some("logs/cli-error.log"));
    }
}

#[test]
fn omitted_process_cli_options_preserve_yaml_values() {
    let (_, conf) = load_server_conf(Some(PROCESS_SERVER_YAML), &[]);
    assert_eq!(conf.threads, 4);
    assert!(conf.work_stealing);
    assert_eq!(conf.grace_period_seconds, Some(40));
    assert_eq!(conf.graceful_shutdown_timeout_seconds, Some(20));
    assert_eq!(conf.upstream_keepalive_pool_size, 256);
    assert_eq!(conf.error_log.as_deref(), Some("logs/file-error.log"));

    let (_, conf) = load_server_conf(
        Some("server:\n  work_stealing: false\n  grace_period_seconds: 0\n  graceful_shutdown_timeout_seconds: 0\n  upstream_keepalive_pool_size: 0\n"),
        &[],
    );
    assert!(!conf.work_stealing);
    assert_eq!(conf.grace_period_seconds, Some(0));
    assert_eq!(conf.graceful_shutdown_timeout_seconds, Some(0));
    assert_eq!(conf.upstream_keepalive_pool_size, 0);
}

#[test]
fn omitted_process_cli_options_without_yaml_keep_constructor_defaults() {
    let (config, conf) = load_server_conf(None, &[]);
    assert!(config.server.threads.is_none());
    assert!(config.server.work_stealing.is_none());
    assert!(config.server.grace_period_seconds.is_none());
    assert!(config.server.graceful_shutdown_timeout_seconds.is_none());
    assert!(config.server.upstream_keepalive_pool_size.is_none());
    assert!(config.server.error_log.is_none());
    assert!(conf.threads >= 1);
    assert!(conf.work_stealing);
    assert_eq!(conf.grace_period_seconds, Some(30));
    assert_eq!(conf.graceful_shutdown_timeout_seconds, Some(10));
    assert_eq!(conf.upstream_keepalive_pool_size, 128);
    assert!(conf.error_log.is_none());
}

#[test]
fn explicit_zero_process_cli_values_override_yaml_before_constructor_fallback() {
    let (_, defaults) = load_server_conf(None, &[]);
    let (config, conf) = load_server_conf(
        Some(PROCESS_SERVER_YAML),
        &[
            "--threads",
            "0",
            "--grace-period",
            "0",
            "--graceful-shutdown-timeout",
            "0",
            "--upstream-keepalive-pool-size",
            "0",
        ],
    );
    assert_eq!(config.server.threads, Some(0));
    assert_eq!(conf.threads, defaults.threads);
    assert!(conf.threads >= 1);
    assert_eq!(conf.grace_period_seconds, Some(0));
    assert_eq!(conf.graceful_shutdown_timeout_seconds, Some(0));
    assert_eq!(conf.upstream_keepalive_pool_size, 0);
    assert!(conf.work_stealing);
    assert_eq!(conf.error_log.as_deref(), Some("logs/file-error.log"));
}

#[test]
fn omitted_and_zero_threads_use_the_same_nonzero_platform_default() {
    let omitted = create_server_conf(&EdgionGatewayConfig::default()).unwrap();
    let mut zero = EdgionGatewayConfig::default();
    zero.server.threads = Some(0);
    let zero = create_server_conf(&zero).unwrap();

    assert_eq!(omitted.threads, zero.threads);
    assert!(omitted.threads >= 1);
}

#[test]
fn positive_thread_count_is_preserved() {
    let mut config = EdgionGatewayConfig::default();
    config.server.threads = Some(7);

    assert_eq!(create_server_conf(&config).unwrap().threads, 7);
}

#[test]
fn pingora_loop_capacity_includes_initial_attempt() {
    let conf = create_server_conf(&EdgionGatewayConfig::default()).unwrap();

    assert_eq!(conf.max_retries, MAX_RETRY_ATTEMPTS as usize + 1);
    assert!(conf.max_retries > crate::ai::runtime::attempt::MAX_AI_BACKENDS);
    assert!(conf.max_retries > usize::from(crate::ai::runtime::attempt::MAX_AI_DISPATCH_ATTEMPTS));
}

#[test]
fn downstream_tls_offload_requires_a_bounded_pair() {
    let mut config = EdgionGatewayConfig::default();
    assert_eq!(
        create_server_conf(&config).unwrap().downstream_tls_offload_threadpool(),
        None
    );

    config.server.downstream_tls_offload_threadpools = Some(2);
    assert!(create_server_conf(&config).is_err());
    config.server.downstream_tls_offload_threadpools = None;
    config.server.downstream_tls_offload_thread_per_pool = Some(2);
    assert!(create_server_conf(&config).is_err());
    config.server.downstream_tls_offload_threadpools = Some(2);
    config.server.downstream_tls_offload_thread_per_pool = Some(2);
    assert_eq!(
        create_server_conf(&config).unwrap().downstream_tls_offload_threadpool(),
        Some((2, 2))
    );

    for invalid in [0, 5, usize::MAX] {
        config.server.downstream_tls_offload_threadpools = Some(invalid);
        assert!(create_server_conf(&config).is_err());
        config.server.downstream_tls_offload_threadpools = Some(2);
        config.server.downstream_tls_offload_thread_per_pool = Some(invalid);
        assert!(create_server_conf(&config).is_err());
        config.server.downstream_tls_offload_thread_per_pool = Some(2);
    }
    config.server.downstream_tls_offload_threadpools = Some(4);
    config.server.downstream_tls_offload_thread_per_pool = Some(4);
    assert_eq!(
        create_server_conf(&config).unwrap().downstream_tls_offload_threadpool(),
        Some((4, 4))
    );
}

#[test]
fn downstream_tls_offload_cli_values_override_file_values() {
    let mut file: EdgionGatewayConfig = yaml_serde::from_str(
        "server:\n  downstream_tls_offload_threadpools: 1\n  downstream_tls_offload_thread_per_pool: 2\n",
    )
    .unwrap();
    let mut cli = EdgionGatewayConfig::default();
    cli.server.downstream_tls_offload_threadpools = Some(3);
    EdgionGatewayConfig::merge(&mut file, &cli);
    assert_eq!(
        create_server_conf(&file).unwrap().downstream_tls_offload_threadpool(),
        Some((3, 2))
    );
}
