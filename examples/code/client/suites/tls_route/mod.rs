// TLS Route Test suite

pub(crate) mod basic;
mod both_absent_parent_ref;
mod multi_sni;
mod proxy_protocol;
mod stream_plugins;

pub use basic::TlsRouteTestSuite;
pub use both_absent_parent_ref::TlsBothAbsentParentRefTestSuite;
pub use multi_sni::TlsMultiSniTestSuite;
pub use proxy_protocol::TlsProxyProtocolTestSuite;
pub use stream_plugins::TlsStreamPluginsTestSuite;
