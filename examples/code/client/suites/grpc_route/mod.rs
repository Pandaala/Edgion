// gRPC Route Test suite
// Note: gRPC TLS moved to EdgionTls module

// Proto module (declared once)
#[path = "../../../proto_gen/test.rs"]
pub mod test;

// Sub-modules - by function
mod basic;
mod grpc_web;
mod r#match;
mod rate_limit;
mod unmatched_route;

// Test suite
pub use basic::GrpcTestSuite;
pub use grpc_web::GrpcWebTestSuite;
pub use r#match::GrpcMatchTestSuite;
pub use rate_limit::GrpcRateLimitTestSuite;
pub use unmatched_route::GrpcUnmatchedRouteTestSuite;
