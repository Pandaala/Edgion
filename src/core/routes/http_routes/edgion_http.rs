use std::sync::Arc;
use std::time::{SystemTime, Duration};
use crate::core::gateway::server_header::ServerHeaderOpts;
use crate::core::routes::DomainRouteRules;
use crate::types::Listener;
use crate::types::EdgionGatewayConfig;
use crate::types::resources::edgion_gateway_config::{ClientTimeout, BackendTimeout};
use crate::core::observe::AccessLogger;

/// Pre-parsed timeout configurations for runtime use
#[derive(Debug, Clone)]
pub struct ParsedTimeouts {
    pub client: ParsedClientTimeout,
    pub backend: ParsedBackendTimeout,
}

#[derive(Debug, Clone)]
pub struct ParsedClientTimeout {
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub keepalive_timeout: u64,  // keepalive takes seconds as u64
}

#[derive(Debug, Clone)]
pub struct ParsedBackendTimeout {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub per_try_timeout: Duration,
    pub idle_timeout: Duration,
}

impl ParsedTimeouts {
    /// Parse timeout configurations from EdgionGatewayConfig
    /// Returns default values if http_timeout is not configured
    pub fn from_config(config: &EdgionGatewayConfig) -> Self {
        match config.spec.http_timeout.as_ref() {
            Some(http_timeout) => Self {
            client: ParsedClientTimeout::from_config(&http_timeout.client),
            backend: ParsedBackendTimeout::from_config(&http_timeout.backend),
            },
            None => Self::default(),
        }
    }
}

impl Default for ParsedTimeouts {
    fn default() -> Self {
        Self {
            client: ParsedClientTimeout::default(),
            backend: ParsedBackendTimeout::default(),
        }
    }
}

impl ParsedClientTimeout {
    fn from_config(config: &ClientTimeout) -> Self {
        use crate::core::utils::parse_duration;
        
        let read_timeout = parse_duration(&config.read_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid read_timeout '{}': {}, using default 60s", config.read_timeout, e);
                Duration::from_secs(60)
            });
        
        let write_timeout = parse_duration(&config.write_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid write_timeout '{}': {}, using default 60s", config.write_timeout, e);
                Duration::from_secs(60)
            });
        
        let keepalive_timeout = parse_duration(&config.keepalive_timeout)
            .map(|d| d.as_secs())
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid keepalive_timeout '{}': {}, using default 75s", config.keepalive_timeout, e);
                75
            });
        
        Self {
            read_timeout,
            write_timeout,
            keepalive_timeout,
        }
    }
}

impl Default for ParsedClientTimeout {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(60),
            keepalive_timeout: 75,
        }
    }
}

impl ParsedBackendTimeout {
    fn from_config(config: &BackendTimeout) -> Self {
        use crate::core::utils::parse_duration;
        
        let connect_timeout = parse_duration(&config.default_connect_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid default_connect_timeout '{}': {}, using default 5s", config.default_connect_timeout, e);
                Duration::from_secs(5)
            });
        
        let request_timeout = parse_duration(&config.default_request_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid default_request_timeout '{}': {}, using default 60s", config.default_request_timeout, e);
                Duration::from_secs(60)
            });
        
        let per_try_timeout = parse_duration(&config.default_per_try_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid default_per_try_timeout '{}': {}, using default 30s", config.default_per_try_timeout, e);
                Duration::from_secs(30)
            });
        
        let idle_timeout = parse_duration(&config.default_idle_timeout)
            .unwrap_or_else(|e| {
                tracing::warn!("Invalid default_idle_timeout '{}': {}, using default 300s", config.default_idle_timeout, e);
                Duration::from_secs(300)
            });
        
        Self {
            connect_timeout,
            request_timeout,
            per_try_timeout,
            idle_timeout,
        }
    }
}

impl Default for ParsedBackendTimeout {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(60),
            per_try_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

pub struct EdgionHttp {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,

    pub listener: Listener,

    pub server_start_time: SystemTime,

    pub server_header_opts: ServerHeaderOpts,
    
    /// Domain routes for this gateway
    pub domain_routes: Arc<DomainRouteRules>,
    
    /// Access logger for writing access logs
    pub access_logger: Arc<AccessLogger>,
    
    /// Global gateway configuration
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    
    /// Pre-parsed timeout configurations (always has default values if not configured)
    pub parsed_timeouts: ParsedTimeouts,
}

