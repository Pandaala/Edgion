use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::{PendingProxyMap, ProxyForwarder};
use crate::core::center::watch_cache::{CenterSyncClient, CenterWatchCacheRegistry};
use crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer;
use anyhow::Result;
use clap::Parser;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Outcome of the federation transport decision at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportDecision {
    /// mTLS with peer-identity binding.
    Mtls,
    /// Plaintext (skip_tls or allow_plaintext); binding skipped.
    Plaintext,
    /// Refuse to start.
    FailClose,
}

/// Pure transport decision.
/// - `tls_resolved`: resolve_active_tls() returned Some
/// - `skip_tls`: the resolved TLS config has skip_tls=true
/// - `allow_plaintext`: the explicit opt-out
/// - `trust_domain`: peer_identity.trust_domain (None or empty = absent)
pub fn decide_transport(
    tls_resolved: bool,
    skip_tls: bool,
    allow_plaintext: bool,
    trust_domain: Option<&str>,
) -> TransportDecision {
    if tls_resolved {
        if skip_tls {
            return TransportDecision::Plaintext;
        }
        match trust_domain {
            Some(td) if !td.trim().is_empty() => TransportDecision::Mtls,
            _ => TransportDecision::FailClose,
        }
    } else if allow_plaintext {
        TransportDecision::Plaintext
    } else {
        TransportDecision::FailClose
    }
}

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    #[arg(short = 'c', long, default_value = "config/edgion-center.yaml")]
    pub config_file: String,
}

impl EdgionCenterCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        // Install a default tracing subscriber before any tracing macro fires
        // (Center emits warnings during config parsing, before any config-driven
        // log setup could run). `set_global_default` is one-shot, so this fixes
        // Center's logging for the whole process.
        crate::core::common::startup::init_default_tracing();

        // Install the process-wide Prometheus recorder before anything emits
        // metrics. Idempotent, so tests and combined modes do not panic on
        // re-install.
        if let Err(e) = crate::core::common::observe::metrics_api::install_global_recorder(
            crate::core::common::observe::metrics_api::RecorderConfig {
                service: "edgion-center",
                idle_timeout_secs: 0,
                histogram_buckets: &[],
            },
        ) {
            tracing::error!(
                component = "center",
                event = "metrics_recorder_install_failed",
                error = %e,
                "Failed to install Prometheus recorder; /metrics will return 500"
            );
        }

        let content = match std::fs::read_to_string(&self.config_file) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    component = "center",
                    path = %self.config_file,
                    error = %e,
                    "Config file not readable, using defaults"
                );
                String::new()
            }
        };
        let config: CenterConfig = if content.is_empty() {
            CenterConfig::default()
        } else {
            let de = serde_yaml::Deserializer::from_str(&content);
            serde_yaml::with::singleton_map_recursive::deserialize(de)
                .map_err(|e| anyhow::anyhow!("Center config parse error ({}): {}", self.config_file, e))?
        };

        let registry = ControllerRegistry::new();
        let aggregator = Arc::new(ResourceAggregator::new());
        let pending_proxies: PendingProxyMap = Arc::new(Mutex::new(HashMap::new()));

        let metadata_store = Arc::new(CenterMetaDataStore::new());
        let sync_client = Arc::new(CenterSyncClient {
            plugin_metadata: CenterWatchCacheRegistry::new(metadata_store.clone()),
        });

        let db: Option<Arc<crate::core::center::db::CenterDb>> = if config.database.enabled {
            match crate::core::center::db::CenterDb::open(&config.database.sqlite_path) {
                Ok(d) => {
                    tracing::info!(component = "center", path = %config.database.sqlite_path, "SQLite DB opened");
                    Some(Arc::new(d))
                }
                Err(e) => {
                    tracing::error!(component = "center", error = %e, "Failed to open SQLite DB, running without persistence");
                    None
                }
            }
        } else {
            None
        };

        // Decide federation transport (fail-close) before constructing the server.
        config
            .grpc_security
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid grpc_security config: {}", e))?;
        let trust_domain = config.peer_identity.as_ref().map(|p| p.trust_domain.clone());
        let transport = {
            let resolved = config.grpc_security.resolve_active_tls();
            decide_transport(
                resolved.is_some(),
                resolved.map(|t| t.skip_tls).unwrap_or(false),
                config.allow_plaintext,
                trust_domain.as_deref(),
            )
        };
        let listener_is_plaintext = match transport {
            TransportDecision::FailClose => {
                return Err(anyhow::anyhow!(
                    "Federation gRPC refuses to start: no TLS configured and allow_plaintext=false \
                     (or mTLS configured without peer_identity.trust_domain). Configure grpc_security \
                     + peer_identity, or set allow_plaintext=true for emergency plaintext."
                ));
            }
            TransportDecision::Plaintext => true,
            TransportDecision::Mtls => false,
        };

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            pending_proxies.clone(),
            config.sync.clone(),
            sync_client.clone(),
            db.clone(),
            listener_is_plaintext,
            trust_domain.clone(),
        );
        let pending_commands = grpc_server.pending_commands.clone();

        let commander = Arc::new(Commander::new(
            registry.clone(),
            pending_commands,
            config.sync.command_timeout_secs,
        ));

        let proxy = Arc::new(ProxyForwarder::new(
            registry.clone(),
            pending_proxies,
            config.sync.command_timeout_secs,
        ));

        let api_state = ApiState {
            aggregator: aggregator.clone(),
            commander,
            proxy: proxy.clone(),
            db: db.clone(),
            metadata_store: metadata_store.clone(),
            sync_client,
            registry: registry.clone(),
            db_required: config.database.enabled,
        };
        let http_addr: std::net::SocketAddr = config.server.http_addr.parse()?;
        let grpc_addr: std::net::SocketAddr = config.server.grpc_addr.parse()?;

        tracing::info!(component = "center", grpc_addr = %grpc_addr, http_addr = %http_addr, "Starting edgion-center");

        // Build gRPC server transport per the decision made above.
        let mut server_builder = tonic::transport::Server::builder()
            .http2_keepalive_interval(Some(Duration::from_secs(10)))
            .http2_keepalive_timeout(Some(Duration::from_secs(5)));
        match transport {
            TransportDecision::Mtls => {
                let tls_cfg = config
                    .grpc_security
                    .resolve_active_tls()
                    .expect("Mtls decision implies resolved TLS config");
                let server_tls = crate::core::common::grpc_tls::load_server_tls(tls_cfg).await?;
                server_builder = server_builder
                    .tls_config(server_tls)
                    .map_err(|e| anyhow::anyhow!("Failed to configure center gRPC TLS: {}", e))?;
                tracing::info!(
                    component = "center",
                    trust_domain = %trust_domain.as_deref().unwrap_or_default(),
                    "Federation gRPC mTLS enabled"
                );
            }
            TransportDecision::Plaintext => {
                tracing::error!(
                    component = "center",
                    "Federation gRPC starting WITHOUT TLS. INSECURE — emergency/migration only!"
                );
                let reason = if config.grpc_security.resolve_active_tls().is_some() {
                    crate::core::common::observe::system_metrics::reasons::SKIP_TLS
                } else {
                    crate::core::common::observe::system_metrics::reasons::PLAINTEXT_DEFAULT
                };
                crate::core::common::observe::system_metrics::mark_system_error(
                    crate::core::common::observe::system_metrics::components::FED_SYNC_SERVER,
                    reason,
                );
            }
            TransportDecision::FailClose => unreachable!("FailClose returned earlier"),
        }

        // gRPC server
        let grpc_handle = tokio::spawn(
            server_builder
                .add_service(FederationSyncServer::new(grpc_server))
                .serve(grpc_addr),
        );

        // HTTP Admin API — unified auth supports both OIDC and local auth.
        // Center does not use the Controller's auto-generated Secret path; credentials come only from operator configuration.
        let auth_config = config.auth.clone();
        let local_auth_config = config.local_auth.clone();

        let http_handle = tokio::spawn(async move {
            let local_auth_intent = local_auth_config.as_ref().is_some_and(|c| c.enabled);
            let oidc_intent = auth_config.as_ref().is_some_and(|c| c.enabled);
            // Safe default: as long as the operator has not explicitly opted
            // out (enabled=false), require auth. In the nothing_configured
            // case, require_auth=true makes the middleware take the 503
            // branch (rather than passthrough), preventing "no configuration"
            // from leaving the admin API exposed.
            let nothing_configured = local_auth_config.is_none() && auth_config.is_none();
            let require_auth = local_auth_intent || oidc_intent || nothing_configured;

            // Defensive observability: WARN on both scenarios where the
            // admin API is not protected by a token.
            // Scenario 1: nothing configured -- middleware takes the 503
            //             fail-close branch.
            // Scenario 2: operator explicitly set enabled=false on all
            //             providers -- middleware passes through. (This is a
            //             legitimate no-auth mode, but it is easy to do by
            //             mistake, so WARN does not imply an error.)
            if nothing_configured {
                tracing::warn!(
                    component = "center",
                    "No authentication configured. Admin endpoints will return 503 \
                     until you configure [local_auth] or [auth] in your YAML."
                );
            } else if !require_auth {
                tracing::warn!(
                    component = "center",
                    "Admin API is running in NO-AUTH mode: all configured providers \
                     have enabled=false. Admin endpoints are OPEN to unauthenticated \
                     requests. Set enabled=true on [local_auth] or [auth] to restore \
                     authentication."
                );
            }

            let auth_state = crate::core::common::unified_auth::UnifiedAuthState::from_configs(
                auth_config.as_ref(),
                local_auth_config.as_ref(),
                require_auth,
                "center",
            )
            .expect("center auth state build failed");
            let base_router = router(api_state);
            // compose hard-codes the assembly order: business routes + auth routes + middleware + CORS.
            // The returned app is final; do not call .route() / .layer() afterwards -- see the function doc.
            let app = crate::core::common::api::compose_admin_routes(base_router, auth_state, local_auth_intent);
            let listener = tokio::net::TcpListener::bind(http_addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });

        // Wait for any task to exit or Ctrl-C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "center", "Ctrl-C received, shutting down");
                Ok(())
            }
            r = grpc_handle => {
                tracing::error!(component = "center", "gRPC server exited: {:?}", r);
                Err(anyhow::anyhow!("federation gRPC server exited unexpectedly"))
            }
            r = http_handle => {
                tracing::error!(component = "center", "HTTP server exited: {:?}", r);
                Err(anyhow::anyhow!("admin HTTP server exited unexpectedly"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decide_transport, TransportDecision};
    use crate::core::center::config::CenterConfig;
    use crate::core::common::unified_auth::UnifiedAuthState;

    #[test]
    fn none_without_optout_fail_closes() {
        assert_eq!(
            decide_transport(false, false, false, None),
            TransportDecision::FailClose
        );
    }
    #[test]
    fn none_with_optout_is_plaintext() {
        assert_eq!(decide_transport(false, false, true, None), TransportDecision::Plaintext);
    }
    #[test]
    fn skip_tls_is_plaintext() {
        assert_eq!(
            decide_transport(true, true, false, Some("edgion.io")),
            TransportDecision::Plaintext
        );
    }
    #[test]
    fn mtls_with_trust_domain_is_mtls() {
        assert_eq!(
            decide_transport(true, false, false, Some("edgion.io")),
            TransportDecision::Mtls
        );
    }
    #[test]
    fn mtls_without_trust_domain_fail_closes() {
        assert_eq!(decide_transport(true, false, false, None), TransportDecision::FailClose);
        assert_eq!(
            decide_transport(true, false, false, Some("")),
            TransportDecision::FailClose
        );
    }

    /// When both local_auth and auth are None on Center:
    /// 1. UnifiedAuthState must be constructed successfully (must not panic).
    /// 2. The safe-default derivation of require_auth must be true (so the
    ///    middleware takes the 503 branch rather than passthrough,
    ///    preventing "no configuration = admin API exposed").
    #[test]
    fn center_boot_with_no_auth_config_does_not_panic() {
        let auth_config: Option<&crate::core::common::auth::AdminAuthConfig> = None;
        let local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig> = None;

        // Safe-default formula (matches the one in run())
        let local_intent = local_auth_config.is_some_and(|c| c.enabled);
        let oidc_intent = auth_config.is_some_and(|c| c.enabled);
        let nothing_configured = local_auth_config.is_none() && auth_config.is_none();
        let require_auth = local_intent || oidc_intent || nothing_configured;

        // When both are None, this must be true; otherwise the middleware would passthrough
        assert!(
            require_auth,
            "require_auth must be true when nothing is configured (safe default)"
        );

        // And state must build successfully (no validate path since both are None)
        let state = UnifiedAuthState::from_configs(auth_config, local_auth_config, require_auth, "test")
            .expect("from_configs must succeed with no auth configs and require_auth=true");
        assert!(state.require_auth);
    }

    /// HTTP-level regression: in the nothing_configured case, business routes must return 503 (not 200 and not 401).
    #[tokio::test]
    async fn center_no_auth_config_admin_routes_return_503() {
        use axum::{body::Body, http::Request, routing::get, Router};
        use std::sync::Arc;
        use tower::ServiceExt;

        // Simulate Center's require_auth derivation at startup (nothing_configured -> true)
        let auth_config: Option<&crate::core::common::auth::AdminAuthConfig> = None;
        let local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig> = None;
        let require_auth = true; // nothing_configured case

        let auth_state: Arc<UnifiedAuthState> =
            UnifiedAuthState::from_configs(auth_config, local_auth_config, require_auth, "test").unwrap();

        let business = Router::new().route("/api/v1/controllers", get(|| async { "controllers" }));
        let app = crate::core::common::api::compose_admin_routes(business, auth_state, false);

        let req = Request::builder()
            .uri("/api/v1/controllers")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            503,
            "admin business route must return 503 when nothing_configured (got {})",
            resp.status()
        );
    }

    /// CenterConfig::default() must carry no admin credentials (no local_auth and no OIDC).
    #[test]
    fn center_default_config_yields_no_auth_config() {
        let config = CenterConfig::default();
        assert!(
            config.local_auth.is_none(),
            "default CenterConfig should not inject edgion1234 via local_auth"
        );
        assert!(
            config.auth.as_ref().is_none_or(|a| !a.enabled),
            "default CenterConfig should not enable OIDC auth"
        );
    }
}
