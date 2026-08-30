mod conf_handler_common;
pub mod dns_cache;
pub mod endpoint_slice;
pub mod services;

pub use dns_cache::{
    decide_external_name_path, get_global_dns_cache, DnsCache, DnsResolutionMode, ExternalNamePath,
    DEFAULT_DNS_CACHE_TTL,
};
pub use endpoint_slice::{create_ep_slice_handler, get_roundrobin_store, EndpointSliceExt, EpSliceStore};
pub use services::{create_service_handler, get_global_service_store, ServiceStore};
