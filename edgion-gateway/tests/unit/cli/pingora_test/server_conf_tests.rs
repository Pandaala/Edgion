use super::*;

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
    assert!(conf.max_retries > crate::ai::attempt::MAX_AI_BACKENDS);
    assert!(conf.max_retries > usize::from(crate::ai::attempt::MAX_AI_DISPATCH_ATTEMPTS));
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
    let mut file: EdgionGatewayConfig = serde_yaml::from_str(
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
