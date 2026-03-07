// HTTP Route Backend test module
// Contains LB policy, weighted backend, timeout and other backend tests

mod health_check;
mod health_check_transition;
mod lb_consistenthash;
mod lb_roundrobin;
mod timeout;
mod weighted_backend;

pub use health_check::HealthCheckTestSuite;
pub use health_check_transition::HealthCheckTransitionTestSuite;
pub use lb_consistenthash::LBConsistentHashTestSuite;
pub use lb_roundrobin::LBRoundRobinTestSuite;
pub use timeout::TimeoutTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
