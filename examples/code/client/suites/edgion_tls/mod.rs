// EdgionTls Test suite
// Contains: https, grpctls, mTLS, cipher, port_only, both_absent_parent_ref, fallback_sni,
//           wildcard_single_label tests

mod both_absent_parent_ref;
mod cipher;
mod fallback_sni;
mod grpctls;
mod https;
mod mtls;
mod port_only;
mod wildcard_single_label;

pub use both_absent_parent_ref::EdgionTlsBothAbsentParentRefTestSuite;
pub use cipher::CipherTestSuite;
pub use fallback_sni::FallbackSniTestSuite;
pub use grpctls::GrpcTlsTestSuite;
pub use https::HttpsTestSuite;
pub use mtls::MtlsTestSuite;
pub use port_only::PortOnlyEdgionTlsTestSuite;
pub use wildcard_single_label::WildcardSingleLabelTestSuite;
