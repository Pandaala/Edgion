// gRPC Route Test suite
// Note: gRPC TLS moved to EdgionTls module

// Proto module (declared once)
#[path = "../../../proto_gen/test.rs"]
pub mod test;

mod backend;
mod basic;
mod delegation;
mod grpc_web;
mod r#match;
mod rate_limit_local;
mod timeout_rewrite_wire;
mod unmatched_route;

pub use backend::GrpcSessionPersistenceTestSuite;
pub use backend::GrpcSpWeightedTestSuite;
pub use basic::GrpcTestSuite;
pub use delegation::{
    GrpcDelegationDynamicInitialTestSuite, GrpcDelegationDynamicUpdateTestSuite, GrpcDelegationTestSuite,
};
pub use grpc_web::GrpcWebTestSuite;
pub use r#match::GrpcMatchTestSuite;
pub use rate_limit_local::GrpcRateLimitLocalTestSuite;
pub use timeout_rewrite_wire::GrpcTimeoutRewriteWireTestSuite;
pub use unmatched_route::GrpcUnmatchedRouteTestSuite;

mod terminal_headers_wire;
pub use terminal_headers_wire::GrpcTerminalHeadersWireTestSuite;
