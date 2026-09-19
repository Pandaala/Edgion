pub mod edgion_gateway;
pub mod gateway_class;

pub use edgion_gateway::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_egc_dns_resolver_default,
    get_remote_ip_header, get_resolved_l4_timeouts, get_resolved_request_body_config, ResolvedDnsResolverDefault,
    ResolvedL4Timeouts,
};
pub use gateway_class::{count_gateway_classes, create_gateway_class_handler};
