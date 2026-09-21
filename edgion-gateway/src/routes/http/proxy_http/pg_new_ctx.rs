use super::EdgionHttpProxy;
use crate::observe::global_metrics;
use crate::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let config = edgion_http.egc.load();
    let mut ctx = EdgionHttpContext::with_gateway_config(config);
    ctx.request_info.listener_port = edgion_http.listener_port;
    ctx.allow_loopback_upstream = ctx.egc.allow_loopback_upstream;
    ctx.request_body_config = ctx.egc.request_body;
    ctx.remote_ip_header = ctx.egc.remote_ip_header.clone();
    ctx.preflight_policy = ctx.egc.preflight_policy;
    ctx.reject_duplicate_host = ctx.egc.reject_duplicate_host;
    global_metrics().ctx_created();
    ctx
}

#[cfg(test)]
mod tests {
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

    fn config(name: &str, mode: PreflightMode, status_code: u16, reject_duplicate_host: bool) -> EdgionGatewayConfig {
        let mut config = EdgionGatewayConfig::new(name, Default::default());
        config.spec.preflight_policy = Some(PreflightPolicy { mode, status_code });
        config.spec.security_protect =
            Some(serde_json::from_value(serde_json::json!({"rejectDuplicateHost": reject_duplicate_host})).unwrap());
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
                        "maxRetries": retries,
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
        assert_eq!(first.egc.max_retries, 2);
        assert_eq!(second.egc.max_retries, 7);
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
        assert_eq!(second.egc.max_retries, 7);
        assert!(second.egc.enable_compression);
    }

    #[test]
    fn request_context_pins_preflight_policy_from_one_snapshot_generation() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
        handler.full_set(&HashMap::from([(
            "policy".to_string(),
            Arc::new(config("policy", PreflightMode::AllOptions, 299, false)),
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
        assert!(!in_flight.reject_duplicate_host);

        handler.partial_update(
            HashMap::new(),
            HashMap::from([(
                "policy".to_string(),
                Arc::new(config("policy", PreflightMode::CorsStandard, 205, true)),
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
        assert!(updated.reject_duplicate_host);
        assert_eq!(
            in_flight.preflight_policy,
            ResolvedPreflightPolicy {
                mode: ResolvedPreflightMode::AllOptions,
                status_code: 299,
            }
        );
        assert!(!in_flight.reject_duplicate_host);

        handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["policy".to_string()]));
        assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
        assert!(new_ctx(&proxy).reject_duplicate_host);
        proxy.egc = crate::config::bind_gateway_config(Some("missing"));
        assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
        assert!(new_ctx(&proxy).reject_duplicate_host);
        proxy.egc = crate::config::bind_gateway_config(None);
        assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
        assert!(new_ctx(&proxy).reject_duplicate_host);

        handler.full_set(&HashMap::new());
    }
}
