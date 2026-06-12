// EdgionTls Test suite
// Contains: https, grpctls, mTLS, mtls_failclose, cipher, port_only,
//           both_absent_parent_ref, fallback_sni, wildcard_single_label tests

mod both_absent_parent_ref;
mod cipher;
mod fallback_sni;
mod grpctls;
mod https;
mod mtls;
mod mtls_failclose;
mod port_only;
mod sni_collision;
mod wildcard_single_label;

pub use both_absent_parent_ref::EdgionTlsBothAbsentParentRefTestSuite;
pub use cipher::CipherTestSuite;
pub use fallback_sni::FallbackSniTestSuite;
pub use grpctls::GrpcTlsTestSuite;
pub use https::HttpsTestSuite;
pub use mtls::MtlsTestSuite;
pub use mtls_failclose::MtlsFailcloseTestSuite;
pub use port_only::PortOnlyEdgionTlsTestSuite;
pub use sni_collision::SniCollisionTestSuite;
pub use wildcard_single_label::WildcardSingleLabelTestSuite;
