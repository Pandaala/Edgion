mod conf_handler_impl;

pub use conf_handler_impl::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_egc_dns_resolver_default,
    get_resolved_request_body_config,
};
