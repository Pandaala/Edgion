// HTTP Route Backend test module
// Contains LB policy, weighted backend, timeout and other backend tests

mod lb_policy;
mod timeout;
mod weighted_backend;

pub use lb_policy::LBPolicyTestSuite;
pub use timeout::TimeoutTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
