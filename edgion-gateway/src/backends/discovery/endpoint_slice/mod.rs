mod conf_handler_impl;
mod ep_slice_store;

pub use conf_handler_impl::create_ep_slice_handler;
pub use ep_slice_store::{get_roundrobin_store, ready_section_sockets, EpSliceStore};
