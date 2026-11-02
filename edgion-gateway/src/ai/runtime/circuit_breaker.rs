//! Request-captured aggregate concurrency cap for one typed EdgionBackend.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct LogicalBackendCircuitBreaker {
    max: u32,
    in_flight: AtomicU32,
}

impl LogicalBackendCircuitBreaker {
    pub fn new(max: u32) -> Arc<Self> {
        Arc::new(Self {
            max,
            in_flight: AtomicU32::new(0),
        })
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn try_acquire(self: Arc<Self>) -> Option<LogicalBackendCircuitBreakerPermit> {
        self.in_flight
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.max).then_some(current.saturating_add(1))
            })
            .ok()
            .map(|_| LogicalBackendCircuitBreakerPermit { state: self })
    }

    #[cfg(test)]
    fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }
}

pub struct LogicalBackendCircuitBreakerPermit {
    state: Arc<LogicalBackendCircuitBreaker>,
}

impl Drop for LogicalBackendCircuitBreakerPermit {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/runtime/circuit_breaker_test.rs"]
mod tests;
