use super::*;
use std::net::{IpAddr, Ipv4Addr};

fn plugin(max_connections: u64) -> ConnectionLimitPlugin {
    ConnectionLimitPlugin::new(&ConnectionLimitConfig { max_connections }).unwrap()
}

fn ctx(layer: StreamLayer, active_connections: u64) -> StreamContext {
    StreamContext::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18080, layer, active_connections)
}

#[tokio::test]
async fn allows_up_to_the_cap_and_denies_beyond_it_on_every_layer() {
    let plugin = plugin(2);
    for layer in [StreamLayer::Stage1, StreamLayer::Tcp] {
        for ordinal in [1, 2] {
            assert!(matches!(
                plugin.on_connection(&ctx(layer, ordinal)).await,
                StreamPluginResult::Allow
            ));
        }
        match plugin.on_connection(&ctx(layer, 3)).await {
            StreamPluginResult::Deny(reason) => assert_eq!(reason, "connection-limit"),
            StreamPluginResult::Allow => panic!("ordinal above the cap must be denied on {layer:?}"),
        }
    }
}

#[tokio::test]
async fn cap_of_one_admits_only_the_first_live_connection() {
    let plugin = plugin(1);
    assert!(matches!(
        plugin.on_connection(&ctx(StreamLayer::Stage1, 1)).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        plugin.on_connection(&ctx(StreamLayer::Stage1, 2)).await,
        StreamPluginResult::Deny(_)
    ));
}

#[test]
fn constructor_rejects_invalid_config() {
    for max_connections in [
        0,
        edgion_resources::resources::edgion_stream_plugins::MAX_CONNECTION_LIMIT + 1,
    ] {
        assert!(ConnectionLimitPlugin::new(&ConnectionLimitConfig { max_connections }).is_err());
    }
    assert_eq!(plugin(5).name(), "ConnectionLimit");
}

#[test]
fn rejections_are_labelled_by_attachment_layer() {
    assert_eq!(
        reject_reason(StreamLayer::Stage1),
        DownstreamConnectionRejectReason::Listener
    );
    assert_eq!(reject_reason(StreamLayer::Tcp), DownstreamConnectionRejectReason::Route);
}
