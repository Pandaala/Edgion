// gRPC Route 测试套件
// 注：gRPC TLS 已移至 EdgionTls 资源模块

// Proto 模块（统一声明一次）
#[path = "../../../proto_gen/test.rs"]
pub mod test;

// 子模块 - 按功能分类
mod basic;
mod r#match;

// 导出测试套件
pub use basic::GrpcTestSuite;
pub use r#match::GrpcMatchTestSuite;
