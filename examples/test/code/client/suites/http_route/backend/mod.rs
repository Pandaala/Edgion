// HTTP Route Backend 测试模块
// 包含负载均衡策略、权重后端、超时等后端相关测试

mod lb_policy;
mod timeout;
mod weighted_backend;

pub use lb_policy::LBPolicyTestSuite;
pub use timeout::TimeoutTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
