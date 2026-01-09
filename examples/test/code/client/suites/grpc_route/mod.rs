// gRPC Route 测试套件

// Proto 模块（统一声明一次）
#[path = "../../../proto_gen/test.rs"]
pub mod test;

mod basic;
mod matches;
mod tls;

pub use basic::GrpcTestSuite;
pub use matches::GrpcMatchTestSuite;
pub use tls::GrpcTlsTestSuite;
