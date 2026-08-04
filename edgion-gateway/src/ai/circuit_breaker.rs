//! Request-captured aggregate concurrency cap for one AI service.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct AiServiceCircuitBreaker {
    max: u32,
    in_flight: AtomicU32,
}

impl AiServiceCircuitBreaker {
    pub fn new(max: u32) -> Arc<Self> {
        Arc::new(Self {
            max,
            in_flight: AtomicU32::new(0),
        })
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn try_acquire(self: &Arc<Self>) -> Option<AiServiceCircuitBreakerPermit> {
        self.in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.max).then_some(current.saturating_add(1))
            })
            .ok()
            .map(|_| AiServiceCircuitBreakerPermit {
                state: Arc::clone(self),
            })
    }

    #[cfg(test)]
    fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }
}

pub struct AiServiceCircuitBreakerPermit {
    state: Arc<AiServiceCircuitBreaker>,
}

impl Drop for AiServiceCircuitBreakerPermit {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_permit_is_bounded_and_released() {
        let state = AiServiceCircuitBreaker::new(1);
        let permit = state.try_acquire().unwrap();
        assert!(state.try_acquire().is_none());
        assert_eq!(state.in_flight(), 1);
        drop(permit);
        assert_eq!(state.in_flight(), 0);
        assert!(state.try_acquire().is_some());
    }
}
