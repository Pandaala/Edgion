// Gateway Test suite
// Note: mTLS moved to EdgionTls module
// Note: EdgionPlugins tests moved to edgion_plugins module

// Sub-modules - by function
mod allowed_routes;
mod combined;
mod dynamic;
mod listener_hostname;
mod port_conflict;
mod real_ip;
mod security;
mod stream_plugins;
mod tls;

// Test suite
pub use allowed_routes::AllowedRoutesAllNamespacesTestSuite;
pub use allowed_routes::AllowedRoutesKindsTestSuite;
pub use allowed_routes::AllowedRoutesSameNamespaceTestSuite;
pub use allowed_routes::AllowedRoutesSelectorNamespaceTestSuite;
pub use combined::CombinedScenariosTestSuite;
pub use dynamic::InitialPhaseTestSuite;
pub use dynamic::UpdatePhaseTestSuite;
pub use listener_hostname::ListenerHostnameTestSuite;
pub use port_conflict::PortConflictTestSuite;
pub use real_ip::RealIpTestSuite;
pub use security::SecurityTestSuite;
pub use stream_plugins::StreamPluginsTestSuite;
pub use tls::BackendTlsTestSuite;
pub use tls::GatewayTlsTestSuite;
