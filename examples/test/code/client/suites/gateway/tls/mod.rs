// Gateway TLS test module
// Note: mTLS moved to EdgionTls module

mod backend_tls;
mod gateway_tls;

pub use backend_tls::BackendTlsTestSuite;
pub use gateway_tls::GatewayTlsTestSuite;
