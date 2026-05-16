//! Optional load balancing algorithm types

/// Load balancing policy types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LbPolicy {
    /// Consistent hashing
    Consistent,
    /// Least connection selection
    LeastConnection,
    /// Exponentially Weighted Moving Average (EWMA) based on response time
    Ewma,
}
