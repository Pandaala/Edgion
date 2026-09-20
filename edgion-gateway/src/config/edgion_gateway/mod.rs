mod conf_handler_impl;

pub(crate) use conf_handler_impl::get_resolved_http_request_config;
pub use conf_handler_impl::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_egc_dns_resolver_default,
    get_resolved_l4_timeouts, ResolvedDnsResolverDefault, ResolvedL4Timeouts,
};
