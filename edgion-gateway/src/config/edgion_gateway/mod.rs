mod conf_handler_impl;

pub use conf_handler_impl::create_edgion_gateway_config_handler;
#[allow(unused_imports)]
pub use conf_handler_impl::{
    count_edgion_gateway_configs, get_edgion_gateway_config_by_name, get_egc_dns_resolver_default,
    list_edgion_gateway_configs,
};
