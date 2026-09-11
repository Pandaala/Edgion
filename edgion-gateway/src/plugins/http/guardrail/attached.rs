//! Request-scoped Guardrail ownership and exactly-once aggregate logging.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use pingora_http::{ResponseHeader, StatusCode};
use pingora_proxy::{ResponseHeadHoldLimits, ResponseHeadOutcome, ResponseHeadReplacement, ResponseHeadUsage};

const GUARDRAIL_REJECTED_JSON: &[u8] = b"{\"error\":{\"message\":\"The response was blocked by a content policy.\",\"type\":\"content_filter_error\",\"code\":\"content_policy_violation\"}}";

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
    head_summary: OnceLock<GuardrailHeadSummary>,
    replacement_header: OnceLock<ResponseHeader>,
    finalized: AtomicBool,
}

#[derive(Clone, Copy)]
struct GuardrailHeadSummary {
    outcome: ResponseHeadOutcome,
    usage: ResponseHeadUsage,
    limits: ResponseHeadHoldLimits,
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
            head_summary: OnceLock::new(),
            replacement_header: OnceLock::new(),
            finalized: AtomicBool::new(false),
        }
    }

    /// Publish the processor's immutable terminal summary. The first terminal
    /// transition wins; no response-window callback takes a blocking lock.
    pub fn publish(&self, summary: GuardrailSummary) {
        let _ = self.summary.set(summary);
    }

    pub(crate) fn publish_head(
        &self,
        outcome: ResponseHeadOutcome,
        usage: ResponseHeadUsage,
        limits: ResponseHeadHoldLimits,
    ) {
        let _ = self.head_summary.set(GuardrailHeadSummary { outcome, usage, limits });
    }

    /// Freeze the local rejection header from the final post-onion response.
    /// Only gateway-owned identity/security fields are retained; origin entity
    /// metadata cannot describe the replacement body.
    pub(crate) fn prepare_head_replacement(
        &self,
        original: &ResponseHeader,
        trace_id: &crate::request_trace_id::RequestTraceId,
    ) {
        let Ok(mut header) = ResponseHeader::build(StatusCode::FORBIDDEN, Some(6)) else {
            return;
        };
        for name in ["server", "strict-transport-security"] {
            if let Some(value) = original.headers.get(name) {
                let _ = header.insert_header(name, value.clone());
            }
        }
        let _ = header.insert_header("content-type", "application/json");
        let _ = header.insert_header("cache-control", "no-store");
        let _ = header.insert_header("content-length", GUARDRAIL_REJECTED_JSON.len().to_string());
        trace_id.reconcile_response(&mut header);
        let _ = self.replacement_header.set(header);
    }

    pub(crate) fn head_replacement(&self) -> Option<ResponseHeadReplacement> {
        Some(ResponseHeadReplacement::new(
            Box::new(self.replacement_header.get()?.clone()),
            vec![bytes::Bytes::from_static(GUARDRAIL_REJECTED_JSON)],
        ))
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
        let mut line = format!(
            "guardrail windows={} passed={} replaced={} input_bytes={} output_bytes={} outcome={} reason={}",
            summary.windows,
            summary.passed,
            summary.replaced,
            summary.input_bytes,
            summary.output_bytes,
            summary.outcome,
            summary.reason
        );
        if let Some(head) = self.head_summary.get() {
            use std::fmt::Write;
            let outcome = match head.outcome {
                ResponseHeadOutcome::Immediate => "immediate",
                ResponseHeadOutcome::Released => "held-released",
                ResponseHeadOutcome::Replaced => "held-replaced",
                ResponseHeadOutcome::Failed(boundary) => boundary.as_str(),
                ResponseHeadOutcome::Cancelled => "cancelled",
                _ => "unknown",
            };
            let _ = write!(
                line,
                " head_outcome={} head_input_bytes={} head_output_bytes={} head_chunks={} head_events={} head_metadata_bytes={} head_work_units={} head_held_ms={} head_limit_input_bytes={} head_limit_output_bytes={} head_limit_chunks={} head_limit_events={} head_limit_metadata_bytes={} head_limit_work_units={} head_timeout_ms={}",
                outcome,
                head.usage.input_bytes(),
                head.usage.output_bytes(),
                head.usage.nonempty_chunks(),
                head.usage.events(),
                head.usage.metadata_bytes(),
                head.usage.work_units(),
                head.usage.held_for().as_millis(),
                head.limits.max_input_bytes(),
                head.limits.max_output_bytes(),
                head.limits.max_nonempty_chunks(),
                head.limits.max_events(),
                head.limits.max_metadata_bytes(),
                head.limits.max_work_units(),
                head.limits.timeout().as_millis(),
            );
        }
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn request_id_held_head_replacement_preserves_custom_echo_and_suppression() {
        let mut ids = crate::request_trace_id::RequestTraceId::default();
        ids.initialize(&http::HeaderMap::new());
        ids.apply_default_patch(&edgion_resources::DefaultIdConfig {
            enabled: Some(false),
            ..Default::default()
        });
        let rule: edgion_resources::RequestIdRule = serde_json::from_value(serde_json::json!({
            "header":"x-correlation-id", "source":{"type":"traceId"}
        }))
        .unwrap();
        let compiled = crate::request_trace_id::CompiledRequestIdRule::new(&rule).unwrap();
        let value = http::HeaderValue::from_str(ids.standard_trace_id().unwrap()).unwrap();
        ids.apply_rule(&compiled, None, Some(value.clone()));
        let mut original = pingora_http::ResponseHeader::build(200, None).unwrap();
        original.insert_header("x-trace-id", "stale").unwrap();
        let owner = super::AttachedGuardrail::new();
        owner.prepare_head_replacement(&original, &ids);
        let replacement = owner.replacement_header.get().unwrap();
        assert_eq!(replacement.headers["x-correlation-id"], value);
        assert!(!replacement.headers.contains_key("x-trace-id"));
        assert!(!replacement.headers.contains_key("traceparent"));
        assert_eq!(replacement.status.as_u16(), 403);
    }

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
