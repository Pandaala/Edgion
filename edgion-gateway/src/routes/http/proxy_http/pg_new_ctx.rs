use super::EdgionHttpProxy;
use crate::observe::global_metrics;
use crate::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = edgion_http.listener_port;
    ctx.allow_loopback_upstream = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|security| security.allow_loopback_upstream)
        .unwrap_or(false);
    let request_config =
        crate::config::get_resolved_http_request_config(edgion_http.http_request_config_name.as_deref());
    ctx.request_body_config = request_config.request_body;
    ctx.remote_ip_header = request_config.remote_ip_header;
    ctx.preflight_policy = request_config.preflight_policy;
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
            edgion_gateway_config: Arc::new(EdgionGatewayConfig::new("listener-copy", Default::default())),
            http_request_config_name: config_name.map(str::to_string),
            parsed_timeouts: Default::default(),
            enable_http2: false,
            real_ip_extractor: None,
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
        proxy.http_request_config_name = Some("missing".to_string());
        assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());
        proxy.http_request_config_name = None;
        assert_eq!(new_ctx(&proxy).preflight_policy, ResolvedPreflightPolicy::default());

        handler.full_set(&HashMap::new());
    }
}
