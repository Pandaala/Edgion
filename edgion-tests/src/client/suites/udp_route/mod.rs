// UDP Route Test suite

mod basic;
mod denied_backend_ref;
mod stream_plugins;

pub use basic::UdpTestSuite;
pub use denied_backend_ref::UdpDeniedBackendRefTestSuite;
pub use stream_plugins::UdpStreamPluginsTestSuite;
