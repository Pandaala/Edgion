//! ForwardAuth plugin module
//!
//! Sends original request metadata to an external authentication service.
//! If the auth service responds with 2xx, the request is forwarded to upstream.
//! Otherwise, the auth service's response is returned to the client.

mod plugin;

pub use plugin::ForwardAuth;
