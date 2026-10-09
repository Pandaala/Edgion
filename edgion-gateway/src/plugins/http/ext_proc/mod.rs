mod attached;
mod client;
mod mutation;
mod plugin;
mod response_streamed;
mod response_trailer;
mod streamed;
#[cfg(test)]
pub(crate) use streamed::{BodyExchange, ExtProcStreamedBodyHandler};

pub(crate) use attached::AttachedExtProc;
pub use plugin::ExtProc;

#[cfg(test)]
include!("../../../../tests/support/plugins/http/ext_proc/mod_test/12_function_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/ext_proc/mod_test/25_const_item.rs");

#[cfg(test)]
pub(crate) use attached::test_body_owner;
