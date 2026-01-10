// gRPC Route Test suite
// Note: gRPC TLS moved to EdgionTls module

// Proto module (declared once)
#[path = "../../../proto_gen/test.rs"]
pub mod test;

// Sub-modules - by function
mod basic;
mod r#match;

// 导出Test suite
pub use basic::GrpcTestSuite;
pub use r#match::GrpcMatchTestSuite;
