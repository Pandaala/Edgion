mod conf_handler_impl;

pub use conf_handler_impl::{
    bind_gateway_config, count_edgion_gateway_configs, create_edgion_gateway_config_handler, default_gateway_config,
    gateway_config_binding_stats, get_egc_dns_resolver_default, GatewayConfigBindingStats, GatewayConfigHandle,
    ResolvedDnsResolverDefault, ResolvedGatewayConfig, ResolvedL4Timeouts,
};
