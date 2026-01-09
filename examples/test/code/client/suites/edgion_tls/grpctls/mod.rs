// EdgionTls gRPC TLS 测试模块

// Proto 模块
#[path = "../../../../proto_gen/test.rs"]
pub mod test;

mod tls;

pub use tls::GrpcTlsTestSuite;
