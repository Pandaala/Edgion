mod service_mgr;
mod conf_handler_impl_service;
mod conf_handler_impl_ep_slice;

pub use service_mgr::{ServiceMgr, UpstreamService, get_global_service_mgr};
pub use conf_handler_impl_service::create_service_mgr_handler;
pub use conf_handler_impl_ep_slice::create_ep_slice_handler;