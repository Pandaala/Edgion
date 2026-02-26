// HTTP Route Backend test module
// Contains LB policy, weighted backend, timeout and other backend tests

mod lb_consistenthash;
mod lb_roundrobin;
mod timeout;
mod weighted_backend;

pub use lb_consistenthash::LBConsistentHashTestSuite;
pub use lb_roundrobin::LBRoundRobinTestSuite;
pub use timeout::TimeoutTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
