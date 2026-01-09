// Gateway TLS 测试模块
// 注：mTLS 已移至 EdgionTls 资源模块

mod backend_tls;

pub use backend_tls::BackendTlsTestSuite;
