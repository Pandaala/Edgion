// TCP Route Test suite

mod basic;
mod denied_backend_ref;
mod geo_restriction;
mod global_connection_ip_restriction;
mod idle_timeout;
mod keepalive_smoke;
mod stream_plugins;

pub use basic::TcpTestSuite;
pub use denied_backend_ref::TcpDeniedBackendRefTestSuite;
pub use geo_restriction::TcpGeoRestrictionTestSuite;
pub use global_connection_ip_restriction::GcirEnforcementTestSuite;
pub use idle_timeout::TcpIdleTimeoutTestSuite;
pub use keepalive_smoke::TcpKeepaliveSmokeTestSuite;
pub use stream_plugins::{TcpEspReloadAllowTestSuite, TcpEspReloadDenyTestSuite, TcpStreamPluginsTestSuite};
