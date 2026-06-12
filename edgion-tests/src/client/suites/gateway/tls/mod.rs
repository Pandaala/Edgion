// Gateway TLS test module
// Note: mTLS moved to EdgionTls module

mod backend_tls;
mod backend_tls_gateway_cert;
mod backend_tls_san;
mod frontend_validation;
mod gateway_tls;
mod misdirected_request;
mod no_hostname_listener;

pub use backend_tls::BackendTlsTestSuite;
pub use backend_tls_gateway_cert::BackendTlsGatewayCertTestSuite;
pub use backend_tls_san::BackendTlsSanTestSuite;
pub use frontend_validation::FrontendValidationTestSuite;
pub use gateway_tls::GatewayTlsTestSuite;
pub use misdirected_request::MisdirectedRequestTestSuite;
pub use no_hostname_listener::GatewayTlsNoHostnameListenerTestSuite;
