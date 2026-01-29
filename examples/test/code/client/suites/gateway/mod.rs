// Gateway Test suite
// Note: mTLS moved to EdgionTls module

// Sub-modules - by function
mod allowed_routes;
mod combined;
mod dynamic;
mod listener_hostname;
mod plugins;
mod port_conflict;
mod real_ip;
mod security;
mod tls;

// 导出Test suite
pub use allowed_routes::AllowedRoutesAllNamespacesTestSuite;
pub use allowed_routes::AllowedRoutesKindsTestSuite;
pub use allowed_routes::AllowedRoutesSameNamespaceTestSuite;
pub use combined::CombinedScenariosTestSuite;
pub use dynamic::InitialPhaseTestSuite;
pub use dynamic::UpdatePhaseTestSuite;
pub use listener_hostname::ListenerHostnameTestSuite;
pub use plugins::PluginLogsTestSuite;
pub use port_conflict::PortConflictTestSuite;
pub use real_ip::RealIpTestSuite;
pub use security::SecurityTestSuite;
pub use tls::BackendTlsTestSuite;
pub use tls::GatewayTlsTestSuite;
