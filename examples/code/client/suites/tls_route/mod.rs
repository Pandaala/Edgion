// TLS Route Test suite

pub(crate) mod basic;
mod multi_sni;
mod proxy_protocol;
mod stream_plugins;

pub use basic::TlsRouteTestSuite;
pub use multi_sni::TlsMultiSniTestSuite;
pub use proxy_protocol::TlsProxyProtocolTestSuite;
pub use stream_plugins::TlsStreamPluginsTestSuite;
