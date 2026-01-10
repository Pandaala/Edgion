// EdgionTls Test suite
// Contains: https, grpctls, mTLS tests

mod grpctls;
mod https;
mod mtls;

pub use grpctls::GrpcTlsTestSuite;
pub use https::HttpsTestSuite;
pub use mtls::MtlsTestSuite;
