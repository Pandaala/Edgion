//! Request-scoped Guardrail ownership and exactly-once aggregate logging.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct GuardrailSummary {
    pub windows: u32,
    pub passed: u32,
    pub replaced: u32,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub outcome: &'static str,
    pub reason: &'static str,
}

impl Default for GuardrailSummary {
    fn default() -> Self {
        Self {
            windows: 0,
            passed: 0,
            replaced: 0,
            input_bytes: 0,
            output_bytes: 0,
            outcome: "pending",
            reason: "none",
        }
    }
}

pub struct AttachedGuardrail {
    summary: OnceLock<GuardrailSummary>,
    finalized: AtomicBool,
}

impl Default for AttachedGuardrail {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachedGuardrail {
    pub fn new() -> Self {
        Self {
            summary: OnceLock::new(),
            finalized: AtomicBool::new(false),
        }
    }

    /// Publish the processor's immutable terminal summary. The first terminal
    /// transition wins; no response-window callback takes a blocking lock.
    pub fn publish(&self, summary: GuardrailSummary) {
        let _ = self.summary.set(summary);
    }

    pub(crate) fn take_summary_line(&self) -> Option<String> {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return None;
        }
        let summary = self.summary.get().cloned().unwrap_or_else(|| GuardrailSummary {
            outcome: "cancelled",
            reason: "logging-fallback",
            ..GuardrailSummary::default()
        });
        Some(format!(
            "guardrail windows={} passed={} replaced={} input_bytes={} output_bytes={} outcome={} reason={}",
            summary.windows,
            summary.passed,
            summary.replaced,
            summary.input_bytes,
            summary.output_bytes,
            summary.outcome,
            summary.reason
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_emitted_exactly_once() {
        let owner = AttachedGuardrail::new();
        owner.publish(GuardrailSummary {
            windows: 2,
            outcome: "pass",
            reason: "normal-eos",
            ..GuardrailSummary::default()
        });
        let first = owner.take_summary_line().unwrap();
        assert!(first.contains("windows=2"));
        assert!(owner.take_summary_line().is_none());
    }
}
