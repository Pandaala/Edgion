use super::*;
use crate::cli::{config::EdgionGatewayConfig, EdgionGatewayCli};
use clap::Parser;

#[test]
fn receive_window_and_stream_limits_are_validated() {
    for value in [0, 65_534, MAX_RECEIVE_WINDOW + 1, u32::MAX] {
        let stream = ServerConfig {
            downstream_h2_stream_window_size: Some(value),
            ..Default::default()
        };
        assert!(DownstreamH2Policy::try_from(&stream).is_err());
        let connection = ServerConfig {
            downstream_h2_connection_window_size: Some(value),
            ..Default::default()
        };
        assert!(DownstreamH2Policy::try_from(&connection).is_err());
    }
    for value in [0, MAX_CONCURRENT_STREAMS + 1, u32::MAX] {
        let config = ServerConfig {
            downstream_h2_max_concurrent_streams: Some(value),
            ..Default::default()
        };
        assert!(DownstreamH2Policy::try_from(&config).is_err());
    }
}

#[test]
fn combined_stream_credit_budget_accepts_the_boundary_and_rejects_excess() {
    let mut config = ServerConfig {
        downstream_h2_stream_window_size: Some(MAX_RECEIVE_WINDOW),
        downstream_h2_connection_window_size: Some(MAX_RECEIVE_WINDOW),
        downstream_h2_max_concurrent_streams: Some(4),
        ..Default::default()
    };
    assert!(DownstreamH2Policy::try_from(&config).is_ok());
    config.downstream_h2_max_concurrent_streams = Some(5);
    let err = DownstreamH2Policy::try_from(&config).err().unwrap();
    assert!(err.to_string().contains("must not exceed 67108864 bytes"));
    config.downstream_h2_stream_window_size = None;
    config.downstream_h2_max_concurrent_streams = Some(MAX_CONCURRENT_STREAMS);
    assert!(DownstreamH2Policy::try_from(&config).is_ok());
    config.downstream_h2_max_concurrent_streams = Some(1);
    config.downstream_h2_connection_window_size = Some(DEFAULT_RECEIVE_WINDOW);
    assert!(DownstreamH2Policy::try_from(&config).is_ok());
}

#[test]
fn cli_overrides_only_explicit_h2_fields() {
    let mut config: EdgionGatewayConfig = serde_yaml::from_str(
        "server:\n  downstream_h2_stream_window_size: 262144\n  downstream_h2_connection_window_size: 1048576\n  downstream_h2_max_concurrent_streams: 100\n",
    )
    .unwrap();
    let omitted = EdgionGatewayCli::try_parse_from(["edgion-gateway"]).unwrap();
    EdgionGatewayConfig::merge(&mut config, &omitted.config);
    assert_eq!(config.server.downstream_h2_stream_window_size, Some(262_144));
    let cli = EdgionGatewayCli::try_parse_from([
        "edgion-gateway",
        "--downstream-h2-stream-window-size",
        "524288",
        "--downstream-h2-max-concurrent-streams",
        "64",
    ])
    .unwrap();
    EdgionGatewayConfig::merge(&mut config, &cli.config);
    assert_eq!(config.server.downstream_h2_stream_window_size, Some(524_288));
    assert_eq!(config.server.downstream_h2_connection_window_size, Some(1_048_576));
    assert_eq!(config.server.downstream_h2_max_concurrent_streams, Some(64));
    let cli = EdgionGatewayCli::try_parse_from(["edgion-gateway", "--downstream-h2-connection-window-size", "2097152"])
        .unwrap();
    EdgionGatewayConfig::merge(&mut config, &cli.config);
    assert_eq!(config.server.downstream_h2_connection_window_size, Some(2_097_152));
}

#[test]
fn invalid_h2_values_fail_during_local_configuration_loading() {
    let config = EdgionGatewayConfig {
        server: ServerConfig {
            downstream_h2_max_concurrent_streams: Some(0),
            ..Default::default()
        },
        ..Default::default()
    };
    let err = EdgionGatewayConfig::load(config).err().unwrap();
    assert_eq!(
        err.to_string(),
        "server.downstream_h2_max_concurrent_streams must be 1..=1024"
    );
}
