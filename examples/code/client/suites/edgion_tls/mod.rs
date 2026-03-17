// EdgionTls Test suite
// Contains: https, grpctls, mTLS, cipher, port_only, both_absent_parent_ref tests

mod both_absent_parent_ref;
mod cipher;
mod grpctls;
mod https;
mod mtls;
mod port_only;

pub use both_absent_parent_ref::EdgionTlsBothAbsentParentRefTestSuite;
pub use cipher::CipherTestSuite;
pub use grpctls::GrpcTlsTestSuite;
pub use https::HttpsTestSuite;
pub use mtls::MtlsTestSuite;
pub use port_only::PortOnlyEdgionTlsTestSuite;
