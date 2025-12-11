use std::sync::Arc;
use std::time::SystemTime;
use crate::core::gateway::server_header::ServerHeaderOpts;
use crate::core::routes::DomainRouteRules;
use crate::types::Listener;
use crate::types::EdgionGatewayConfig;
use crate::core::observe::AccessLogger;

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
    pub access_logger: Option<Arc<AccessLogger>>,
    
    /// Global gateway configuration
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

