// HTTP Route 测试套件
// 注：HTTPS 已移至 EdgionTls 资源模块

// 基础测试
mod basic;

// 子模块 - 按功能分类
mod backend;
mod filters;
mod r#match;
mod protocol;

// 导出基础测试
pub use basic::HttpTestSuite;

// 导出子模块测试
pub use backend::{LBPolicyTestSuite, TimeoutTestSuite, WeightedBackendTestSuite};
pub use filters::{HttpRedirectTestSuite, HttpSecurityTestSuite};
pub use protocol::WebSocketTestSuite;
pub use r#match::HttpMatchTestSuite;
