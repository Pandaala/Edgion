use super::*;
use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK;
use crate::observe::AccessLogger;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::proxy_http::preflight_handler::{ResolvedPreflightMode, ResolvedPreflightPolicy};
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use edgion_resources::resources::edgion_gateway_config::{PreflightMode, PreflightPolicy};
use edgion_resources::{EdgionGatewayConfig, ListenerKey};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn proxy(config_name: Option<&str>) -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(AccessLogger::default()),
        egc: crate::config::bind_gateway_config(config_name),

        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
    }
}

fn config(name: &str, mode: PreflightMode, status_code: u16) -> EdgionGatewayConfig {
    let mut config = EdgionGatewayConfig::new(name, Default::default());
    config.spec.preflight_policy = Some(PreflightPolicy { mode, status_code });
    config
}

#[test]
fn requests_pin_security_timeouts_and_server_policy_across_update_and_delete() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
    let make_config = |enabled: bool, limit: usize, retries: u32, timeout: &str| {
        Arc::new(
            serde_json::from_value::<EdgionGatewayConfig>(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionGatewayConfig",
                "metadata": {"name": "http-generation"},
                "spec": {
                    "retry": {"attempts": retries},
                    "securityProtect": {"xForwardedForLimit": limit,
                        "requireSniHostMatch": enabled, "allowLoopbackUpstream": enabled},
                    "server": {"enableCompression": enabled, "downstreamKeepaliveRequestLimit": retries},
                    "httpTimeout": {"backend": {"defaultConnectTimeout": timeout}}
                }
            }))
            .unwrap(),
        )
    };
    handler.full_set(&HashMap::from([(
        "http-generation".into(),
        make_config(false, 500, 2, "3s"),
    )]));
    let proxy = proxy(Some("http-generation"));
    let first = new_ctx(&proxy);
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("http-generation".into(), make_config(true, 20, 7, "9s"))]),
        HashSet::new(),
    );
    let second = new_ctx(&proxy);
    assert_eq!(first.egc.x_forwarded_for_limit, Some(500));
    assert_eq!(second.egc.x_forwarded_for_limit, Some(20));
    assert!(!first.egc.require_sni_host_match);
    assert!(second.egc.require_sni_host_match);
    assert!(!first.allow_loopback_upstream);
    assert!(second.allow_loopback_upstream);
    assert!(!first.egc.enable_compression);
    assert!(second.egc.enable_compression);
    assert_eq!(first.egc.retry_attempts, 2);
    assert_eq!(second.egc.retry_attempts, 7);
    assert_eq!(
        first.egc.parsed_timeouts.backend.connect_timeout,
        std::time::Duration::from_secs(3)
    );
    assert_eq!(
        second.egc.parsed_timeouts.backend.connect_timeout,
        std::time::Duration::from_secs(9)
    );
    handler.partial_update(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["http-generation".into()]),
    );
    let defaults = new_ctx(&proxy);
    assert!(!defaults.allow_loopback_upstream);
    assert_eq!(defaults.egc.x_forwarded_for_limit, None);
    assert_eq!(second.egc.retry_attempts, 7);
    assert!(second.egc.enable_compression);
}

#[test]
fn request_context_pins_preflight_policy_from_one_snapshot_generation() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
    handler.full_set(&HashMap::from([(
        "policy".to_string(),
        Arc::new(config("policy", PreflightMode::AllOptions, 299)),
    )]));
    let mut proxy = proxy(Some("policy"));

    let in_flight = new_ctx(&proxy);
    assert_eq!(
        in_flight.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::AllOptions,
            status_code: 299,
        }
    );

    handler.partial_update(
        HashMap::new(),
        HashMap::from([(
            "policy".to_string(),
            Arc::new(config("policy", PreflightMode::CorsStandard, 205)),
        )]),
        HashSet::new(),
    );
    let updated = new_ctx(&proxy);
    assert_eq!(
        updated.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::CorsStandard,
            status_code: 205,
        }
    );
    assert_eq!(
        in_flight.preflight_policy,
        ResolvedPreflightPolicy {
            mode: ResolvedPreflightMode::AllOptions,
            status_code: 299,
        }
    );

    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["policy".to_string()]));
    assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
    proxy.egc = crate::config::bind_gateway_config(Some("missing"));
    assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
    proxy.egc = crate::config::bind_gateway_config(None);
    assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());

    handler.full_set(&HashMap::new());
}

#[tokio::test]
async fn remote_ip_consumers_pin_header_across_update_and_delete() {
    use super::super::pg_upstream_request_filter::upstream_request_filter;
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::plugins::runtime::traits::session::PluginSession;
    use tokio::io::AsyncWriteExt;

    let (proxy, first, second, defaults) = {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
        let make_config = |header: &str| {
            let mut config = EdgionGatewayConfig::new("remote-ip-generation", Default::default());
            config.spec.forwarded_headers.remote_ip_header = header.to_string();
            Arc::new(config)
        };
        handler.full_set(&HashMap::from([(
            "remote-ip-generation".into(),
            make_config("X-Initial-Remote-IP"),
        )]));
        let proxy = proxy(Some("remote-ip-generation"));
        let first = new_ctx(&proxy);
        handler.partial_update(
            HashMap::new(),
            HashMap::from([("remote-ip-generation".into(), make_config("X-Updated-Remote-IP"))]),
            HashSet::new(),
        );
        let second = new_ctx(&proxy);
        handler.partial_update(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["remote-ip-generation".into()]),
        );
        let defaults = new_ctx(&proxy);
        handler.full_set(&HashMap::new());
        (proxy, first, second, defaults)
    };

    // Exercise all generations only after publication and deletion have completed.
    for (mut ctx, expected) in [
        (first, "x-initial-remote-ip"),
        (second, "x-updated-remote-ip"),
        (defaults, "x-real-ip"),
    ] {
        let (mut client, server) = tokio::io::duplex(4096);
        let mut session = pingora_proxy::Session::new_h1(Box::new(server));
        client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
        assert!(session.read_request().await.unwrap());
        ctx.request_info.remote_ip = "192.0.2.10".into();
        ctx.request_info.downstream_peer_ip = "192.0.2.20".into();
        {
            let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert!(adapter.is_remote_ip_header(&expected.to_ascii_uppercase()));
            for name in ["x-initial-remote-ip", "x-updated-remote-ip", "x-real-ip"] {
                assert_eq!(adapter.is_remote_ip_header(name), name == expected);
            }
        }
        let mut upstream = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        upstream.insert_header(expected, "spoofed").unwrap();
        upstream_request_filter(&proxy, &mut session, &mut upstream, &mut ctx)
            .await
            .unwrap();
        assert_eq!(upstream.headers.get(expected).unwrap(), "192.0.2.10");
        assert_eq!(upstream.headers.get("x-forwarded-for").unwrap(), "192.0.2.20");
        for name in ["x-initial-remote-ip", "x-updated-remote-ip", "x-real-ip"] {
            assert_eq!(upstream.headers.contains_key(name), name == expected);
        }
    }
}
