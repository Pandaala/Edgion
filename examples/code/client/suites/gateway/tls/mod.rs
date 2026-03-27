// Gateway TLS test module
// Note: mTLS moved to EdgionTls module

mod backend_tls;
mod backend_tls_san;
mod frontend_validation;
mod gateway_tls;
mod no_hostname_listener;

pub use backend_tls::BackendTlsTestSuite;
pub use backend_tls_san::BackendTlsSanTestSuite;
pub use frontend_validation::FrontendValidationTestSuite;
pub use gateway_tls::GatewayTlsTestSuite;
pub use no_hostname_listener::GatewayTlsNoHostnameListenerTestSuite;
