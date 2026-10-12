pub mod edgion_gateway;
pub mod gateway_class;

pub use edgion_gateway::{bind_gateway_config, default_gateway_config, GatewayConfigHandle, ResolvedGatewayConfig};
pub use edgion_gateway::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_egc_dns_resolver_default,
    ResolvedDnsResolverDefault, ResolvedL4Timeouts,
};
pub use gateway_class::create_gateway_class_handler;
