//! Typed producer outcomes for the rateLimitExceeded condition, isolated per invocation.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateLimitLocalOutcome {
    #[default]
    NotEvaluated,
    WithinLimit,
    Exceeded,
}

/// Opaque restoration token for a nested invocation.
pub struct RateLimitLocalFrame {
    base: usize,
    previous: Option<(usize, usize)>,
}

#[derive(Default)]
pub(crate) struct RateLimitLocalOutcomes {
    values: Vec<RateLimitLocalOutcome>,
    current: Option<(usize, usize)>,
}

impl RateLimitLocalOutcomes {
    pub(crate) fn begin(&mut self, count: usize) -> RateLimitLocalFrame {
        let base = self.values.len();
        self.values.resize(base + count, RateLimitLocalOutcome::NotEvaluated);
        let previous = self.current.replace((base, count));
        RateLimitLocalFrame { base, previous }
    }

    pub(crate) fn end(&mut self, frame: RateLimitLocalFrame) {
        self.values.truncate(frame.base);
        self.current = frame.previous;
    }

    pub(crate) fn get(&self, slot: usize) -> RateLimitLocalOutcome {
        let Some((base, count)) = self.current else {
            return RateLimitLocalOutcome::NotEvaluated;
        };
        if slot >= count {
            return RateLimitLocalOutcome::NotEvaluated;
        }
        self.values.get(base + slot).copied().unwrap_or_default()
    }

    pub(crate) fn set(&mut self, slot: usize, outcome: RateLimitLocalOutcome) -> bool {
        let Some((base, count)) = self.current else {
            return false;
        };
        if slot >= count {
            return false;
        }
        let Some(value) = self.values.get_mut(base + slot) else {
            return false;
        };
        *value = outcome;
        true
    }
}

#[cfg(test)]
mod tests;
