// HTTP Route Backend test module
// Contains LB policy, weighted backend, timeout and other backend tests

mod circuit_breaker;
mod grpc_health_check;
mod health_check;
mod health_check_transition;
mod lb_consistenthash;
mod lb_roundrobin;
mod partially_invalid;
mod retry_on_status;
mod session_persistence;
mod timeout;
mod weighted_backend;

pub use circuit_breaker::CircuitBreakerTestSuite;
pub use grpc_health_check::GrpcHealthCheckTestSuite;
pub use health_check::HealthCheckTestSuite;
pub use health_check_transition::HealthCheckTransitionTestSuite;
pub use lb_consistenthash::LBConsistentHashTestSuite;
pub use lb_roundrobin::LBRoundRobinTestSuite;
pub use partially_invalid::PartiallyInvalidTestSuite;
pub use retry_on_status::RetryOnStatusTestSuite;
pub use session_persistence::SessionPersistenceTestSuite;
pub use timeout::TimeoutTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
