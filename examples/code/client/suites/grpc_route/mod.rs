// gRPC Route Test suite
// Note: gRPC TLS moved to EdgionTls module

// Proto module (declared once)
#[path = "../../../proto_gen/test.rs"]
pub mod test;

// Sub-modules - by function
mod basic;
mod r#match;
mod unmatched_route;

// Test suite
pub use basic::GrpcTestSuite;
pub use r#match::GrpcMatchTestSuite;
pub use unmatched_route::GrpcUnmatchedRouteTestSuite;
