pub mod edgion_gateway;
pub mod gateway_class;

pub(crate) use edgion_gateway::get_resolved_http_request_config;
pub use edgion_gateway::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_egc_dns_resolver_default,
    get_resolved_l4_timeouts, ResolvedDnsResolverDefault, ResolvedL4Timeouts,
};
pub use gateway_class::{count_gateway_classes, create_gateway_class_handler};
