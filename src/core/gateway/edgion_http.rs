use std::sync::Arc;
use std::time::SystemTime;
use crate::core::gateway::server_header::ServerHeaderOpts;
use crate::core::routes::DomainRouteRules;
use crate::types::Listener;
use crate::core::observe::GatewayMetrics;
use crate::core::observe::AccessLogger;

pub struct EdgionHttp {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,

    pub listener: Listener,

    pub server_start_time: SystemTime,

    pub server_header_opts: ServerHeaderOpts,

    /// Gateway metrics (thread-safe, high-performance)
    pub metrics: GatewayMetrics,
    
    /// Domain routes for this gateway
    pub domain_routes: Arc<DomainRouteRules>,
    
    /// Access logger for writing access logs
    pub access_logger: Option<Arc<AccessLogger>>,
}

