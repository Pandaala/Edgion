// Gateway 测试套件
// 注：mTLS 已移至 EdgionTls 资源模块

// 子模块 - 按功能分类
mod plugins;
mod real_ip;
mod security;
mod tls;

// 导出测试套件
pub use plugins::PluginLogsTestSuite;
pub use real_ip::RealIpTestSuite;
pub use security::SecurityTestSuite;
pub use tls::BackendTlsTestSuite;
