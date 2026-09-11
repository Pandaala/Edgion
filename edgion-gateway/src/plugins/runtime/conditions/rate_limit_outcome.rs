//! Typed producer outcomes for the rateLimitExceeded condition, isolated per invocation.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateLimitOutcome {
    #[default]
    NotEvaluated,
    WithinLimit,
    Exceeded,
}

/// Opaque restoration token for a nested invocation.
pub struct RateLimitFrame {
    base: usize,
    previous: Option<(usize, usize)>,
}

#[derive(Default)]
pub(crate) struct RateLimitOutcomes {
    values: Vec<RateLimitOutcome>,
    current: Option<(usize, usize)>,
}

impl RateLimitOutcomes {
    pub(crate) fn begin(&mut self, count: usize) -> RateLimitFrame {
        let base = self.values.len();
        self.values.resize(base + count, RateLimitOutcome::NotEvaluated);
        let previous = self.current.replace((base, count));
        RateLimitFrame { base, previous }
    }

    pub(crate) fn end(&mut self, frame: RateLimitFrame) {
        self.values.truncate(frame.base);
        self.current = frame.previous;
    }

    pub(crate) fn get(&self, slot: usize) -> RateLimitOutcome {
        let Some((base, count)) = self.current else {
            return RateLimitOutcome::NotEvaluated;
        };
        if slot >= count {
            return RateLimitOutcome::NotEvaluated;
        }
        self.values.get(base + slot).copied().unwrap_or_default()
    }

    pub(crate) fn set(&mut self, slot: usize, outcome: RateLimitOutcome) -> bool {
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
