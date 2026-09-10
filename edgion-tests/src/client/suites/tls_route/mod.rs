// TLS Route Test suite

pub(crate) mod basic;
mod both_absent_parent_ref;
mod denied_backend_ref;
mod geo_restriction;
mod multi_sni;
mod passthrough;
mod proxy_protocol;
mod stream_plugins;
mod unknown_kind_backend;

pub use basic::TlsRouteTestSuite;
pub use both_absent_parent_ref::TlsBothAbsentParentRefTestSuite;
pub use denied_backend_ref::TlsDeniedBackendRefTestSuite;
pub use geo_restriction::TlsGeoRestrictionTestSuite;
pub use multi_sni::TlsMultiSniTestSuite;
pub use passthrough::TlsPassthroughTestSuite;
pub use proxy_protocol::TlsProxyProtocolTestSuite;
pub(crate) use stream_plugins::stream_plugins::probe_tls_policy;
pub use stream_plugins::TlsStreamPluginsTestSuite;
pub use unknown_kind_backend::TlsUnknownKindBackendTestSuite;
