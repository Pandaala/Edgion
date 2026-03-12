// Gateway TLS test module
// Note: mTLS moved to EdgionTls module

mod backend_tls;
mod gateway_tls;
mod no_hostname_listener;

pub use backend_tls::BackendTlsTestSuite;
pub use gateway_tls::GatewayTlsTestSuite;
pub use no_hostname_listener::GatewayTlsNoHostnameListenerTestSuite;
