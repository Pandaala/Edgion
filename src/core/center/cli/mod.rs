use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::{PendingProxyMap, ProxyForwarder};
use crate::core::center::watch_cache::{CenterSyncClient, CenterWatchCacheRegistry};
use crate::core::common::config::ConfSyncSecurityConfig;
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
    /// Refuse to start.
    FailClose,
}

/// Pure transport decision. Federation requires mTLS; anything else fail-closes.
/// - `sec`: the federation gRPC security config
/// - `trust_domain`: peer_identity.trust_domain (None or empty = absent)
pub fn decide_transport(sec: &ConfSyncSecurityConfig, trust_domain: Option<&str>) -> TransportDecision {
    match sec.resolve_mtls_or_refuse() {
        Err(_) => TransportDecision::FailClose,
        Ok(_) => match trust_domain {
            Some(td) if !td.trim().is_empty() => TransportDecision::Mtls,
            _ => TransportDecision::FailClose,
        },
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

        // Validate admin TLS paths up front (cheap, sync) before spawning listeners,
        // mirroring the grpc_security.validate()? gate above.
        if let Some(tls) = config.server.admin_tls.as_ref() {
            tls.validate()
                .map_err(|e| anyhow::anyhow!("Invalid admin_tls config: {}", e))?;
        }

        // Fail-fast: reject malformed allow_admin_ips CIDRs before spawning listeners.
        crate::core::common::api::ip_allowlist::validate_admin_ips(&config.server.allow_admin_ips)?;
        crate::core::common::api::ip_acceptor::validate_tcp_ips(&config.server.allow_tcp_ips)?;
        if !config.server.allow_tcp_ips.is_empty() && config.server.admin_tls.is_none() {
            tracing::warn!(
                component = "center",
                "allow_tcp_ips is set but admin_tls is disabled; the L4 filter has no effect"
            );
        }

        let trust_domain = config.peer_identity.as_ref().map(|p| p.trust_domain.clone());
        let transport = decide_transport(&config.grpc_security, trust_domain.as_deref());
        if transport == TransportDecision::FailClose {
            return Err(anyhow::anyhow!(
                "Federation gRPC refuses to start: federation requires mTLS. \
                 Configure grpc_security (certs + ca) and peer_identity.trust_domain."
            ));
        }

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            pending_proxies.clone(),
            config.sync.clone(),
            sync_client.clone(),
            db.clone(),
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
        let probe_addr: std::net::SocketAddr = config.server.probe_addr.parse()?;
        let metrics_addr: std::net::SocketAddr = config.server.metrics_addr.parse()?;

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
        let admin_tls = config.server.admin_tls.clone();
        let allow_admin_ips = config.server.allow_admin_ips.clone();
        let allow_tcp_ips = config.server.allow_tcp_ips.clone();
        let web_dir = config.web.dir.clone();
        // Startup diagnostic only — does not change cookie runtime behavior.
        if let Some(local) = config.local_auth.as_ref() {
            // Args are (admin_tls_present, cookie_secure) — keep this order.
            let admin_tls_present = admin_tls.is_some();
            if let Some(msg) = admin_tls_cookie_warning(admin_tls_present, local.cookie_secure) {
                tracing::warn!(component = "center", "{}", msg);
            }
        }

        let api_state_for_probe = api_state.clone();
        let http_handle = tokio::spawn(async move {
            let local_auth_intent = local_auth_config.as_ref().is_some_and(|c| c.enabled);
            // Authentication is mandatory; no-auth mode was removed. require_auth is
            // always true. In the nothing_configured case, the middleware takes the
            // 503 fail-close branch preventing "no configuration" from leaving the
            // admin API exposed.
            let require_auth = true;

            let nothing_configured = local_auth_config.is_none() && auth_config.is_none();
            // Scenario 1: nothing configured -- middleware takes the 503 fail-close branch.
            if nothing_configured {
                tracing::warn!(
                    component = "center",
                    "No authentication configured. Admin endpoints will return 503 \
                     until you configure [local_auth] or [auth] in your YAML."
                );
            }
            // Scenario 2: operator explicitly set enabled=false on a provider -- auth is
            // still mandatory; this config is no longer supported and the flag is ignored.
            let local_explicitly_disabled = local_auth_config.as_ref().is_some_and(|c| !c.enabled);
            let oidc_explicitly_disabled = auth_config.as_ref().is_some_and(|c| !c.enabled);
            if local_explicitly_disabled || oidc_explicitly_disabled {
                tracing::warn!(
                    component = "center",
                    "auth `enabled: false` is no longer supported; authentication is mandatory. \
                     Configure a real local_auth/auth provider. Ignoring the disable flag."
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
            // Dashboard UI: a public SPA fallback mounted AFTER compose_admin_routes.
            // Because it is added after compose returns its final (auth-wrapped) router,
            // the fallback is NOT covered by unified_auth — so the login page and its
            // JS/CSS load pre-auth, while every registered /api route (added before)
            // stays auth-protected. The fallback only ever receives unmatched paths.
            // Mounted only when an asset source resolves (embedded feature on, or
            // web.dir/EDGION_WEB_DIR set); otherwise Center runs in pure-API mode.
            let app = match crate::core::center::api::web::WebSource::resolve(web_dir.as_deref()) {
                Some(source) => {
                    let source: std::sync::Arc<crate::core::center::api::web::WebSource> = std::sync::Arc::new(source);
                    tracing::info!(component = "center", "dashboard UI hosting enabled");
                    app.fallback(move |uri: axum::http::Uri| {
                        let source = source.clone();
                        async move { crate::core::center::api::web::serve(source, uri).await }
                    })
                }
                None => {
                    tracing::info!(component = "center", "dashboard UI hosting disabled (pure-API mode)");
                    app
                }
            };
            // admin-api-02: outermost IP allowlist — rejects unauthorized peers before auth.
            // Not mounted when the allowlist is empty (allow-all, backward compatible).
            let app = match crate::core::common::api::ip_allowlist::build_admin_ip_matcher(&allow_admin_ips)? {
                Some(m) => {
                    tracing::info!(
                        component = "center",
                        entries = allow_admin_ips.len(),
                        "admin IP allowlist active"
                    );
                    app.layer(axum::middleware::from_fn_with_state(
                        m,
                        crate::core::common::api::ip_allowlist::ip_allowlist_middleware,
                    ))
                }
                None => {
                    tracing::info!(component = "center", "admin IP allowlist inactive (allow all)");
                    app
                }
            };
            // Shared make-service with connect-info, required by the IP allowlist
            // (ConnectInfo<SocketAddr>) and used by both HTTP and HTTPS serve branches.
            let make_service = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
            match admin_tls {
                Some(tls) => {
                    let cert_path = tls.cert_path();
                    let key_path = tls.key_path();
                    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
                        .await
                        .map_err(|e| {
                            tracing::error!(component = "center", error = %e, "Failed to load admin TLS cert/key");
                            anyhow::anyhow!("admin TLS load failed: {}", e)
                        })?;
                    // Logged before serve() binds the socket; a bind failure (e.g. address
                    // in use) is reported by the error returned from serve() below.
                    tracing::info!(component = "center", addr = %http_addr, "Center admin API HTTPS enabled");
                    // admin-api-04: L4 pre-filter — when allow_tcp_ips is set, reject
                    // unauthorized peers before the rustls handshake. When empty, serve via
                    // the original bind_rustls path (zero overhead, behavior unchanged).
                    match crate::core::common::api::ip_acceptor::build_tcp_ip_matcher(&allow_tcp_ips)? {
                        Some(m) => {
                            tracing::info!(
                                component = "center",
                                entries = allow_tcp_ips.len(),
                                "L4 TCP IP pre-filter active (HTTPS)"
                            );
                            let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(rustls_config)
                                .acceptor(crate::core::common::api::ip_acceptor::TcpIpAcceptor::new(m));
                            axum_server::bind(http_addr)
                                .acceptor(acceptor)
                                .serve(make_service)
                                .await
                                .map_err(|e| anyhow::anyhow!("admin HTTPS server error: {}", e))?;
                        }
                        None => {
                            axum_server::bind_rustls(http_addr, rustls_config)
                                .serve(make_service)
                                .await
                                .map_err(|e| anyhow::anyhow!("admin HTTPS server error: {}", e))?;
                        }
                    }
                }
                None => {
                    let listener = tokio::net::TcpListener::bind(http_addr)
                        .await
                        .map_err(|e| anyhow::anyhow!("admin HTTP bind error: {}", e))?;
                    tracing::info!(component = "center", addr = %http_addr, "Center admin API HTTP enabled (no TLS)");
                    axum::serve(listener, make_service)
                        .await
                        .map_err(|e| anyhow::anyhow!("admin HTTP server error: {}", e))?;
                }
            }
            Ok::<(), anyhow::Error>(())
        });

        // Dedicated probe listener (liveness/readiness) — own socket, no auth.
        let _probe_handle = tokio::spawn(async move {
            let app = crate::core::center::api::create_probe_router(api_state_for_probe);
            match tokio::net::TcpListener::bind(probe_addr).await {
                Ok(listener) => {
                    let _ = axum::serve(listener, app).await;
                }
                Err(e) => {
                    tracing::error!(component = "center", error = %e, addr = %probe_addr, "Center probe listener failed to bind")
                }
            }
        });

        // Dedicated metrics listener (Prometheus scrape) — own socket, no auth.
        let _metrics_handle = tokio::spawn(async move {
            let app = crate::core::center::api::create_metrics_router();
            match tokio::net::TcpListener::bind(metrics_addr).await {
                Ok(listener) => {
                    let _ = axum::serve(listener, app).await;
                }
                Err(e) => {
                    tracing::error!(component = "center", error = %e, addr = %metrics_addr, "Center metrics listener failed to bind")
                }
            }
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
                // The admin listener may be HTTP or HTTPS depending on admin_tls.
                tracing::error!(component = "center", "admin server exited: {:?}", r);
                Err(anyhow::anyhow!("admin server exited unexpectedly"))
            }
        }
    }
}

/// Decide which admin-TLS / cookie_secure startup warning to emit, if any.
///
/// Pure (no logging) so it is unit-testable without a tracing capture layer.
/// Returns `None` when the combination is safe. Only meaningful when local auth
/// is configured (otherwise no cookie is ever issued — the caller gates on that).
fn admin_tls_cookie_warning(admin_tls_present: bool, cookie_secure: bool) -> Option<&'static str> {
    match (admin_tls_present, cookie_secure) {
        // Center itself terminates HTTPS but the auth cookie is non-Secure.
        (true, false) => Some(
            "admin_tls is enabled but local_auth.cookie_secure=false: the auth cookie is issued \
             without the Secure attribute, so browsers may transmit it back over plain HTTP. \
             Set cookie_secure=true.",
        ),
        // Secure cookie but plain-HTTP listener: a direct http:// browser reach silently
        // drops the session cookie. Fine behind a TLS-terminating proxy (warn-only).
        (false, true) => Some(
            "local_auth.cookie_secure=true but admin_tls is not configured: if Center is reached \
             directly over http:// the browser will drop the session cookie. Ensure a \
             TLS-terminating proxy fronts the admin listener.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{admin_tls_cookie_warning, decide_transport, TransportDecision};
    use crate::core::center::config::CenterConfig;
    use crate::core::common::config::{ConfSyncSecurityConfig, ConfSyncTlsConfig};
    use crate::core::common::unified_auth::UnifiedAuthState;

    fn mtls_sec() -> ConfSyncSecurityConfig {
        ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn none_fail_closes() {
        let sec = ConfSyncSecurityConfig::default();
        assert_eq!(decide_transport(&sec, None), TransportDecision::FailClose);
    }

    #[test]
    fn skip_tls_fail_closes() {
        let sec = ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: true,
            }),
            ..Default::default()
        };
        assert_eq!(decide_transport(&sec, Some("edgion.io")), TransportDecision::FailClose);
    }

    #[test]
    fn mtls_with_trust_domain_is_mtls() {
        assert_eq!(
            decide_transport(&mtls_sec(), Some("edgion.io")),
            TransportDecision::Mtls
        );
    }

    #[test]
    fn mtls_without_trust_domain_fail_closes() {
        assert_eq!(decide_transport(&mtls_sec(), None), TransportDecision::FailClose);
        assert_eq!(decide_transport(&mtls_sec(), Some("")), TransportDecision::FailClose);
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

    #[test]
    fn test_admin_tls_cookie_warning_combinations() {
        // Contradiction: Center terminates HTTPS but cookie is non-Secure -> warn.
        // Assert a substring UNIQUE to this arm so a swapped match arm would fail the test.
        // ("admin_tls is enabled" appears only here; "cookie_secure=true" appears in BOTH arms.)
        let msg = admin_tls_cookie_warning(true, false).expect("contradiction case should warn");
        assert!(msg.contains("admin_tls is enabled"), "unexpected message: {msg}");
        // Silent-drop risk: Secure cookie but admin listener is plain HTTP -> warn.
        // "admin_tls is not configured" is unique to this arm.
        let msg = admin_tls_cookie_warning(false, true).expect("silent-drop case should warn");
        assert!(msg.contains("admin_tls is not configured"), "unexpected message: {msg}");
        // Safe: HTTPS + Secure cookie.
        assert!(admin_tls_cookie_warning(true, true).is_none());
        // Safe: plain HTTP + non-Secure cookie (consistent dev setup).
        assert!(admin_tls_cookie_warning(false, false).is_none());
    }

    #[tokio::test]
    async fn test_rustls_config_loads_self_signed_pem() {
        use rcgen::{CertificateParams, KeyPair};
        // ServerConfig needs a process CryptoProvider; main installs it, tests must too.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let key_pair = KeyPair::generate().expect("rcgen key generation failed");
        let params = CertificateParams::new(vec!["localhost".to_string()]).expect("rcgen params failed");
        let cert = params.self_signed(&key_pair).expect("rcgen self-sign failed");

        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("server.crt");
        let key_path = dir.path().join("server.key");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();

        let res = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await;
        assert!(res.is_ok(), "valid self-signed PEM should load: {:?}", res.err());
    }

    #[tokio::test]
    async fn test_rustls_config_rejects_malformed_pem() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("bad.crt");
        let key_path = dir.path().join("bad.key");
        std::fs::write(&cert_path, b"not a pem at all").unwrap();
        std::fs::write(&key_path, b"also not a key").unwrap();

        let res = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await;
        assert!(res.is_err(), "malformed PEM must return Err, not panic");
    }

    #[tokio::test]
    async fn test_rustls_config_rejects_missing_path() {
        let res =
            axum_server::tls_rustls::RustlsConfig::from_pem_file("/no/such/dir/cert.crt", "/no/such/dir/key.key").await;
        assert!(res.is_err(), "missing cert/key files must return Err, not panic");
    }
}
