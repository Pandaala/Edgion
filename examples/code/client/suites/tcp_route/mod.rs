// TCP Route Test suite

// Sub-modules - by function
mod basic;
mod global_connection_ip_restriction;
mod idle_timeout;
mod stream_plugins;

// Test suite
pub use basic::TcpTestSuite;
pub use global_connection_ip_restriction::GcirEnforcementTestSuite;
pub use idle_timeout::TcpIdleTimeoutTestSuite;
pub use stream_plugins::{TcpEspReloadAllowTestSuite, TcpEspReloadDenyTestSuite, TcpStreamPluginsTestSuite};
