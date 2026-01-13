// Gateway Test suite
// Note: mTLS moved to EdgionTls module

// Sub-modules - by function
mod plugins;
mod real_ip;
mod security;
mod tls;

// 导出Test suite
pub use plugins::PluginLogsTestSuite;
pub use real_ip::RealIpTestSuite;
pub use security::SecurityTestSuite;
pub use tls::BackendTlsTestSuite;
pub use tls::GatewayTlsTestSuite;
