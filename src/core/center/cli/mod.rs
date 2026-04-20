use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::{PendingProxyMap, ProxyForwarder};
use crate::core::center::scheduler;
use crate::core::center::watch_cache::{CenterSyncClient, CenterWatchCacheRegistry};
use crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer;
use parking_lot::Mutex;

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
        let config: CenterConfig = {
            let de = serde_yaml::Deserializer::from_str(&content);
            match serde_yaml::with::singleton_map_recursive::deserialize(de) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        component = "center",
                        path = %self.config_file,
                        error = %e,
                        "Config parse error, using defaults"
                    );
                    CenterConfig::default()
                }
            }
        };

        let registry = ControllerRegistry::new();
        let aggregator = Arc::new(ResourceAggregator::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

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

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            pending_proxies.clone(),
            config.sync.clone(),
            sync_client.clone(),
            db.clone(),
        );
        let pending_lists = grpc_server.pending_lists.clone();
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
        };
        let http_addr: std::net::SocketAddr = config.server.http_addr.parse()?;
        let grpc_addr: std::net::SocketAddr = config.server.grpc_addr.parse()?;

        tracing::info!(component = "center", grpc_addr = %grpc_addr, http_addr = %http_addr, "Starting edgion-center");

        // Validate security config early
        config
            .grpc_security
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid grpc_security config: {}", e))?;

        // Build gRPC server with optional TLS
        let mut server_builder = tonic::transport::Server::builder();
        if let Some(tls_cfg) = config.grpc_security.resolve_active_tls() {
            if tls_cfg.skip_tls {
                tracing::error!(
                    component = "center",
                    "grpc_security skip_tls=true: starting WITHOUT TLS. INSECURE, emergency use only!"
                );
            } else {
                let server_tls = crate::core::common::grpc_tls::load_server_tls(tls_cfg).await?;
                server_builder = server_builder
                    .tls_config(server_tls)
                    .map_err(|e| anyhow::anyhow!("Failed to configure center gRPC TLS: {}", e))?;
                tracing::info!(component = "center", "Federation gRPC mTLS enabled");
            }
        }

        // gRPC server
        let grpc_handle = tokio::spawn(
            server_builder
                .add_service(FederationSyncServer::new(grpc_server))
                .serve(grpc_addr),
        );

        // HTTP Admin API — unified auth supports both OIDC and local auth.
        // Center 不走 Controller 的 Secret 自动生成路径，凭证只来自 operator 配置。
        let auth_config = config.auth.clone();
        let local_auth_config = config.local_auth.clone();

        let http_handle = tokio::spawn(async move {
            let local_auth_intent = local_auth_config.as_ref().is_some_and(|c| c.enabled);
            let oidc_intent = auth_config.as_ref().is_some_and(|c| c.enabled);
            // Safe default：只要 operator 没有显式 opt out（enabled=false），就要求 auth。
            // nothing_configured 场景下 require_auth=true 让 middleware 走 503 分支
            // （而非 passthrough），防止"什么都不配"= 管理 API 裸奔。
            let nothing_configured = local_auth_config.is_none() && auth_config.is_none();
            let require_auth = local_auth_intent || oidc_intent || nothing_configured;

            // Defensive observability：两种"管理 API 没被 token 保护"的场景都要 WARN。
            // 场景 1：什么都没配——middleware 走 503 fail-close 分支
            // 场景 2：operator 显式 enabled=false 关掉了所有 provider——middleware 直通
            //         （这是合法 no-auth 模式，但容易误操作，所以 WARN 不等于错误）
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
            // compose 内部固定装配顺序：业务路由 + auth 路由 + middleware + CORS。
            // 返回的 app 视为终态，不可再 .route() / .layer()——见函数 doc。
            let app = crate::core::common::api::compose_admin_routes(base_router, auth_state, local_auth_intent);
            let listener = tokio::net::TcpListener::bind(http_addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });

        // Scheduler
        let sched_handle = tokio::spawn(scheduler::run(
            registry.clone(),
            pending_lists,
            config.sync.clone(),
            shutdown_rx.clone(),
        ));

        // Wait for any task to exit or Ctrl-C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "center", "Ctrl-C received, shutting down");
                let _ = shutdown_tx.send(true);
            }
            r = grpc_handle => { tracing::error!(component = "center", "gRPC server exited: {:?}", r); }
            r = http_handle => { tracing::error!(component = "center", "HTTP server exited: {:?}", r); }
            r = sched_handle => { tracing::error!(component = "center", "Scheduler exited: {:?}", r); }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::core::center::config::CenterConfig;
    use crate::core::common::unified_auth::UnifiedAuthState;

    /// Center 在 local_auth 和 auth 都为 None 时：
    /// 1. 应能成功构造 UnifiedAuthState，不应 panic
    ///    （fix #01 round 2 的 bug 1 回归——旧 or_else 会造 LocalAuthConfig 走 validate）
    /// 2. safe-default require_auth 推导结果应为 true
    ///    （保证 middleware 走 503 分支而非 passthrough，防止"什么都不配 = admin API 裸奔"）
    #[test]
    fn center_boot_with_no_auth_config_does_not_panic() {
        let auth_config: Option<&crate::core::common::auth::AdminAuthConfig> = None;
        let local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig> = None;

        // Safe-default 公式（和 run() 里一致）
        let local_intent = local_auth_config.is_some_and(|c| c.enabled);
        let oidc_intent = auth_config.is_some_and(|c| c.enabled);
        let nothing_configured = local_auth_config.is_none() && auth_config.is_none();
        let require_auth = local_intent || oidc_intent || nothing_configured;

        // 两者都 None 时必为 true，否则 middleware 会 passthrough
        assert!(
            require_auth,
            "require_auth must be true when nothing is configured (safe default)"
        );

        // 且能成功构造 state（无 validate 路径，因为都是 None）
        let state = UnifiedAuthState::from_configs(auth_config, local_auth_config, require_auth, "test")
            .expect("from_configs must succeed with no auth configs and require_auth=true");
        assert!(state.require_auth);
    }

    /// HTTP 级回归：nothing_configured 场景下，业务路由应返回 503（不是 200 也不是 401）。
    /// 这是 fix #01 follow-up 的语义回归——旧版 4fae592 用 require_auth=false
    /// 导致 admin API 裸奔。
    #[tokio::test]
    async fn center_no_auth_config_admin_routes_return_503() {
        use axum::{body::Body, http::Request, routing::get, Router};
        use std::sync::Arc;
        use tower::ServiceExt;

        // 模拟 Center 启动时的 require_auth 推导（nothing_configured → true）
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

    /// CenterConfig::default() 应该不带任何 admin 凭证，确保 fix #01 删除
    /// edgion1234 fallback 后没有遗漏的默认注入路径。
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
