pub mod endpoint;
pub mod endpoint_slice;
pub mod services;

pub use endpoint::{
    create_endpoint_handler, get_endpoint_roundrobin_store, EndpointDiscovery, EndpointExt, EndpointStore,
};
pub use endpoint_slice::{create_ep_slice_handler, get_roundrobin_store, EndpointSliceExt, EpSliceStore};
pub use services::{create_service_handler, get_global_service_store, ServiceStore};
