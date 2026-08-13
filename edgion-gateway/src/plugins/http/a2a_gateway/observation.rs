//! Bounded, read-only A2A response observation.

use bytes::Bytes;
use serde::de::{IgnoredAny, Visitor};
use serde::{Deserialize, Deserializer};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::sse::{SseBlockStatus, SseDecoder, SseRecordRef};

const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_JSON_OBSERVATION_MEMORY_BYTES: usize = 64 * MAX_JSON_RESPONSE_BYTES;
static JSON_OBSERVATION_MEMORY_BYTES: AtomicUsize = AtomicUsize::new(0);

struct JsonBudgetSlot {
    counter: &'static AtomicUsize,
}

impl JsonBudgetSlot {
    fn acquire(counter: &'static AtomicUsize) -> Option<Self> {
        let mut current = counter.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(MAX_JSON_RESPONSE_BYTES)?;
            if next > MAX_JSON_OBSERVATION_MEMORY_BYTES {
                return None;
            }
            match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Self { counter }),
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for JsonBudgetSlot {
    fn drop(&mut self) {
        self.counter.fetch_sub(MAX_JSON_RESPONSE_BYTES, Ordering::AcqRel);
    }
}

/// Final response media selected after response filters and retry arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2aResponseMedia {
    Json,
    Sse,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pending,
    Success,
    Error,
    Unknown,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    None,
    Task,
    Message,
    StatusUpdate,
    ArtifactUpdate,
    Other,
}

impl ResultKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Task => "task",
            Self::Message => "message",
            Self::StatusUpdate => "status-update",
            Self::ArtifactUpdate => "artifact-update",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorClass {
    None,
    Standard,
    A2a,
    Other,
}

impl ErrorClass {
    const fn from_code(code: i64) -> Self {
        match code {
            -32700 | -32603..=-32600 => Self::Standard,
            -32099..=-32001 => Self::A2a,
            _ => Self::Other,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Standard => "jsonrpc",
            Self::A2a => "a2a",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    None,
    Unspecified,
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
    Rejected,
    AuthRequired,
    Unknown,
}

impl TaskState {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled | Self::Rejected)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unspecified => "unspecified",
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::InputRequired => "input-required",
            Self::Rejected => "rejected",
            Self::AuthRequired => "auth-required",
            Self::Unknown => "unknown",
        }
    }
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TaskStateVisitor;

        impl Visitor<'_> for TaskStateVisitor {
            type Value = TaskState;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an A2A task-state string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(match value {
                    "TASK_STATE_UNSPECIFIED" => TaskState::Unspecified,
                    "TASK_STATE_SUBMITTED" => TaskState::Submitted,
                    "TASK_STATE_WORKING" => TaskState::Working,
                    "TASK_STATE_COMPLETED" => TaskState::Completed,
                    "TASK_STATE_FAILED" => TaskState::Failed,
                    "TASK_STATE_CANCELED" => TaskState::Canceled,
                    "TASK_STATE_INPUT_REQUIRED" => TaskState::InputRequired,
                    "TASK_STATE_REJECTED" => TaskState::Rejected,
                    "TASK_STATE_AUTH_REQUIRED" => TaskState::AuthRequired,
                    _ => TaskState::Unknown,
                })
            }
        }

        deserializer.deserialize_str(TaskStateVisitor)
    }
}

#[derive(Deserialize)]
struct JsonRpcEnvelope {
    jsonrpc: JsonRpcVersion,
    #[serde(default)]
    result: Option<ResultObservation>,
    #[serde(default)]
    error: Option<ErrorObservation>,
}

#[derive(Debug)]
struct JsonRpcVersion;

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VersionVisitor;

        impl Visitor<'_> for VersionVisitor {
            type Value = JsonRpcVersion;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the JSON-RPC 2.0 version")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value == "2.0" {
                    Ok(JsonRpcVersion)
                } else {
                    Err(E::custom("unsupported JSON-RPC version"))
                }
            }
        }

        deserializer.deserialize_str(VersionVisitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResultObservation {
    #[serde(default)]
    task: Option<TaskObservation>,
    #[serde(default)]
    message: Option<IgnoredAny>,
    #[serde(default)]
    status_update: Option<StatusUpdateObservation>,
    #[serde(default)]
    artifact_update: Option<IgnoredAny>,
    #[serde(default)]
    status: Option<StatusObservation>,
}

#[derive(Deserialize)]
struct TaskObservation {
    #[serde(default)]
    status: Option<StatusObservation>,
}

#[derive(Deserialize)]
struct StatusUpdateObservation {
    status: StatusObservation,
}

#[derive(Deserialize)]
struct StatusObservation {
    state: TaskState,
}

#[derive(Deserialize)]
struct ErrorObservation {
    code: i64,
}

#[derive(Debug, Clone, Copy)]
struct Aggregate {
    outcome: Outcome,
    reason: &'static str,
    result_kind: ResultKind,
    error_class: ErrorClass,
    task_state: TaskState,
    terminal_state_seen: bool,
    events: u32,
    malformed: u32,
    uncertain: bool,
}

impl Default for Aggregate {
    fn default() -> Self {
        Self {
            outcome: Outcome::Pending,
            reason: "none",
            result_kind: ResultKind::None,
            error_class: ErrorClass::None,
            task_state: TaskState::None,
            terminal_state_seen: false,
            events: 0,
            malformed: 0,
            uncertain: false,
        }
    }
}

impl Aggregate {
    fn observe_envelope(&mut self, envelope: JsonRpcEnvelope) {
        if self.outcome == Outcome::Error {
            self.mark_unknown("event-after-error");
            return;
        }
        let _ = envelope.jsonrpc;
        match (envelope.result, envelope.error) {
            (Some(result), None) => {
                let Ok((kind, state)) = classify_result(result) else {
                    self.mark_unknown("invalid-result");
                    return;
                };
                self.events = self.events.saturating_add(1);
                self.outcome = Outcome::Success;
                self.reason = "normal-eos";
                self.result_kind = kind;
                self.error_class = ErrorClass::None;
                self.observe_state(state);
            }
            (None, Some(error)) => {
                self.events = self.events.saturating_add(1);
                self.outcome = Outcome::Error;
                self.reason = "jsonrpc-error";
                self.result_kind = ResultKind::None;
                self.error_class = ErrorClass::from_code(error.code);
            }
            _ => self.mark_unknown("invalid-envelope"),
        }
    }

    fn observe_state(&mut self, state: TaskState) {
        if self.terminal_state_seen || state == TaskState::None {
            return;
        }
        self.task_state = state;
        self.terminal_state_seen = state.is_terminal();
    }

    fn mark_unknown(&mut self, reason: &'static str) {
        self.malformed = self.malformed.saturating_add(1);
        self.uncertain = true;
        self.reason = reason;
    }

    fn finish(&mut self, empty_reason: &'static str) {
        if self.uncertain {
            self.outcome = Outcome::Unknown;
        } else if self.outcome == Outcome::Pending {
            self.outcome = Outcome::Unknown;
            self.reason = empty_reason;
        }
    }
}

fn classify_result(result: ResultObservation) -> Result<(ResultKind, TaskState), ()> {
    let ResultObservation {
        task,
        message,
        status_update,
        artifact_update,
        status,
    } = result;
    let members = usize::from(task.is_some())
        + usize::from(message.is_some())
        + usize::from(status_update.is_some())
        + usize::from(artifact_update.is_some())
        + usize::from(status.is_some());
    if members > 1 {
        return Err(());
    }
    if members == 0 {
        return Ok((ResultKind::Other, TaskState::None));
    }
    if let Some(task) = task {
        return Ok((
            ResultKind::Task,
            task.status.map_or(TaskState::None, |status| status.state),
        ));
    }
    if message.is_some() {
        return Ok((ResultKind::Message, TaskState::None));
    }
    if let Some(update) = status_update {
        return Ok((ResultKind::StatusUpdate, update.status.state));
    }
    if artifact_update.is_some() {
        return Ok((ResultKind::ArtifactUpdate, TaskState::None));
    }
    Ok((ResultKind::Task, status.map_or(TaskState::None, |status| status.state)))
}

enum BodyObserver {
    Pending,
    Json {
        bytes: Vec<u8>,
        oversized: bool,
        _budget: JsonBudgetSlot,
    },
    Sse(SseDecoder),
    Other,
}

/// Request-scoped A2A response owner. It is moved into the HTTP context and
/// mutated serially by the canonical response driver and logging boundary.
pub struct AttachedA2aObservation {
    body: BodyObserver,
    aggregate: Aggregate,
    finalized: bool,
    emitted: bool,
}

impl Default for AttachedA2aObservation {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachedA2aObservation {
    pub fn new() -> Self {
        Self {
            body: BodyObserver::Pending,
            aggregate: Aggregate::default(),
            finalized: false,
            emitted: false,
        }
    }

    /// Select the final response media. JSON storage is allocated once here,
    /// outside the per-chunk callback.
    pub fn observe_final_header(&mut self, media: A2aResponseMedia) {
        if self.finalized || !matches!(self.body, BodyObserver::Pending) {
            return;
        }
        self.body = match media {
            A2aResponseMedia::Json => {
                let Some(budget) = JsonBudgetSlot::acquire(&JSON_OBSERVATION_MEMORY_BYTES) else {
                    self.aggregate.mark_unknown("observation-budget");
                    self.body = BodyObserver::Other;
                    return;
                };
                let mut bytes = Vec::new();
                if bytes.try_reserve_exact(MAX_JSON_RESPONSE_BYTES).is_err() {
                    self.aggregate.mark_unknown("observation-budget");
                    self.body = BodyObserver::Other;
                    return;
                }
                BodyObserver::Json {
                    bytes,
                    oversized: false,
                    _budget: budget,
                }
            }
            A2aResponseMedia::Sse => BodyObserver::Sse(SseDecoder::with_default_limit()),
            A2aResponseMedia::Other => BodyObserver::Other,
        };
    }

    /// Observe a final forwarded chunk without changing or retaining its
    /// source handle. Natural EOS settles the owner exactly once.
    pub fn observe_chunk(&mut self, chunk: Option<&Bytes>, end_of_stream: bool) {
        if self.finalized {
            return;
        }
        match &mut self.body {
            BodyObserver::Pending => self.aggregate.mark_unknown("missing-header"),
            BodyObserver::Other => {}
            BodyObserver::Json { bytes, oversized, .. } => {
                if let Some(chunk) = chunk {
                    if !*oversized {
                        if bytes
                            .len()
                            .checked_add(chunk.len())
                            .is_some_and(|length| length <= MAX_JSON_RESPONSE_BYTES)
                        {
                            bytes.extend_from_slice(chunk);
                        } else {
                            bytes.clear();
                            *oversized = true;
                        }
                    }
                }
            }
            BodyObserver::Sse(decoder) => {
                if let Some(chunk) = chunk {
                    let aggregate = &mut self.aggregate;
                    decoder.feed_records(chunk.clone(), |record| observe_sse_record(aggregate, record));
                    if decoder.degraded() {
                        aggregate.mark_unknown("oversized-event");
                    }
                }
            }
        }
        if end_of_stream {
            self.finish_natural();
        }
    }

    fn finish_natural(&mut self) {
        if self.finalized {
            return;
        }
        let body = std::mem::replace(&mut self.body, BodyObserver::Other);
        match body {
            BodyObserver::Pending => self.aggregate.mark_unknown("missing-header"),
            BodyObserver::Other => self.aggregate.finish("unsupported-media"),
            BodyObserver::Json { bytes, oversized, .. } => {
                if oversized {
                    self.aggregate.mark_unknown("oversized-body");
                } else {
                    match serde_json::from_slice::<JsonRpcEnvelope>(&bytes) {
                        Ok(envelope) => self.aggregate.observe_envelope(envelope),
                        Err(_) => self.aggregate.mark_unknown("malformed-json"),
                    }
                }
                self.aggregate.finish("empty-body");
            }
            BodyObserver::Sse(mut decoder) => {
                let aggregate = &mut self.aggregate;
                decoder.finish_records(|record| observe_sse_record(aggregate, record));
                if decoder.degraded() {
                    aggregate.mark_unknown("oversized-event");
                } else if decoder.malformed() {
                    aggregate.uncertain = true;
                    aggregate.reason = "malformed-sse";
                }
                aggregate.finish("empty-stream");
            }
        }
        self.finalized = true;
    }

    /// Settle an abort/cancellation/timeout path. A previously completed
    /// observation always wins.
    pub fn terminalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        if self.finalized {
            return;
        }
        // Release a JSON reservation at the transport terminal boundary, not
        // later when the access log happens to drain this owner.
        self.body = BodyObserver::Other;
        self.aggregate.outcome = if outcome == "error" {
            Outcome::Error
        } else {
            Outcome::Unknown
        };
        self.aggregate.reason = reason;
        self.finalized = true;
    }

    /// Drain the one redacted static aggregate line into the existing plugin
    /// log. Raw payload and protocol identifiers are never retained here.
    pub fn take_summary_line(&mut self) -> Option<String> {
        if self.emitted {
            return None;
        }
        if !self.finalized {
            self.terminalize_incomplete("unknown", "logging-fallback");
        }
        self.emitted = true;
        Some(format!(
            "a2a_resp outcome={} reason={} kind={} error={} state={} events={} malformed={}",
            self.aggregate.outcome.as_str(),
            self.aggregate.reason,
            self.aggregate.result_kind.as_str(),
            self.aggregate.error_class.as_str(),
            self.aggregate.task_state.as_str(),
            self.aggregate.events,
            self.aggregate.malformed
        ))
    }
}

fn observe_sse_record(aggregate: &mut Aggregate, record: SseRecordRef<'_>) {
    match record {
        SseRecordRef::Block {
            status: SseBlockStatus::Data,
            event,
        } => match event.deserialize_data::<JsonRpcEnvelope>() {
            Ok(envelope) => aggregate.observe_envelope(envelope),
            Err(_) => aggregate.mark_unknown("malformed-event"),
        },
        SseRecordRef::Block {
            status: SseBlockStatus::Poisoned,
            ..
        }
        | SseRecordRef::Opaque(_) => aggregate.mark_unknown("malformed-sse"),
        SseRecordRef::EosTail(tail) if tail.len() != 0 => aggregate.mark_unknown("partial-event"),
        SseRecordRef::Preamble(_)
        | SseRecordRef::FramingContinuation(_)
        | SseRecordRef::EosTail(_)
        | SseRecordRef::Block {
            status: SseBlockStatus::Dataless,
            ..
        } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_budget_is_hard_bounded_and_raii_recoverable() {
        static TEST_BUDGET: AtomicUsize = AtomicUsize::new(0);
        let mut slots = Vec::new();
        for _ in 0..(MAX_JSON_OBSERVATION_MEMORY_BYTES / MAX_JSON_RESPONSE_BYTES) {
            slots.push(JsonBudgetSlot::acquire(&TEST_BUDGET).expect("slot within budget"));
        }
        assert_eq!(TEST_BUDGET.load(Ordering::Acquire), MAX_JSON_OBSERVATION_MEMORY_BYTES);
        assert!(JsonBudgetSlot::acquire(&TEST_BUDGET).is_none());
        slots.pop();
        assert_eq!(
            TEST_BUDGET.load(Ordering::Acquire),
            MAX_JSON_OBSERVATION_MEMORY_BYTES - MAX_JSON_RESPONSE_BYTES
        );
        slots.push(JsonBudgetSlot::acquire(&TEST_BUDGET).expect("released slot must be reusable"));
        drop(slots);
        assert_eq!(TEST_BUDGET.load(Ordering::Acquire), 0);
    }

    fn json_owner() -> AttachedA2aObservation {
        let mut owner = AttachedA2aObservation::new();
        owner.observe_final_header(A2aResponseMedia::Json);
        owner
    }

    fn sse_owner() -> AttachedA2aObservation {
        let mut owner = AttachedA2aObservation::new();
        owner.observe_final_header(A2aResponseMedia::Sse);
        owner
    }

    #[test]
    fn json_task_success_is_observed_across_every_split() {
        let body = Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":"private-id","result":{"task":{"status":{"state":"TASK_STATE_COMPLETED"},"secret":"private"}}}"#,
        );
        for split in 0..=body.len() {
            let mut owner = json_owner();
            owner.observe_chunk(Some(&body.slice(..split)), false);
            owner.observe_chunk(Some(&body.slice(split..)), true);
            let line = owner.take_summary_line().expect("summary");
            assert!(line.contains("outcome=success"), "{split}: {line}");
            assert!(line.contains("kind=task"), "{split}: {line}");
            assert!(line.contains("state=completed"), "{split}: {line}");
            assert!(!line.contains("private"));
        }
    }

    #[test]
    fn direct_task_and_message_results_are_classified() {
        let mut task = json_owner();
        task.observe_chunk(
            Some(&Bytes::from_static(
                br#"{"jsonrpc":"2.0","result":{"status":{"state":"TASK_STATE_WORKING"}}}"#,
            )),
            true,
        );
        let task_line = task.take_summary_line().unwrap();
        assert!(task_line.contains("kind=task"));
        assert!(task_line.contains("state=working"));

        let mut message = json_owner();
        message.observe_chunk(
            Some(&Bytes::from_static(
                br#"{"jsonrpc":"2.0","result":{"message":{"parts":[]}}}"#,
            )),
            true,
        );
        assert!(message.take_summary_line().unwrap().contains("kind=message"));
    }

    #[test]
    fn json_error_codes_are_reduced_to_static_classes() {
        for (code, class) in [(-32700, "jsonrpc"), (-32601, "jsonrpc"), (-32001, "a2a"), (7, "other")] {
            let body = Bytes::from(format!(
                r#"{{"jsonrpc":"2.0","id":"do-not-log","error":{{"code":{code},"message":"secret","data":{{"token":"hidden"}}}}}}"#
            ));
            let mut owner = json_owner();
            owner.observe_chunk(Some(&body), true);
            let line = owner.take_summary_line().unwrap();
            assert!(line.contains("outcome=error"));
            assert!(line.contains(&format!("error={class}")));
            assert!(!line.contains("secret"));
            assert!(!line.contains("hidden"));
            assert!(!line.contains("do-not-log"));
        }
    }

    #[test]
    fn invalid_partial_and_ambiguous_json_are_unknown() {
        for body in [
            br#"{"jsonrpc":"2.0","result":{"message":{}}"#.as_slice(),
            br#"{"jsonrpc":"1.0","result":{"message":{}}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","result":{},"error":{"code":-32600}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","result":{},"result":{}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","result":{"task":{},"message":{}}}"#.as_slice(),
        ] {
            let mut owner = json_owner();
            owner.observe_chunk(Some(&Bytes::copy_from_slice(body)), true);
            assert!(owner.take_summary_line().unwrap().contains("outcome=unknown"));
        }
    }

    #[test]
    fn json_cap_is_inclusive_and_never_parses_an_oversized_prefix() {
        let prefix = br#"{"jsonrpc":"2.0","result":{"message":{}},"padding":""#;
        let suffix = br#""}"#;
        let padding_len = MAX_JSON_RESPONSE_BYTES - prefix.len() - suffix.len();
        let mut exact_body = Vec::with_capacity(MAX_JSON_RESPONSE_BYTES);
        exact_body.extend_from_slice(prefix);
        exact_body.extend(std::iter::repeat_n(b'x', padding_len));
        exact_body.extend_from_slice(suffix);
        assert_eq!(exact_body.len(), MAX_JSON_RESPONSE_BYTES);

        let mut exact = json_owner();
        exact.observe_chunk(Some(&Bytes::from(exact_body.clone())), false);
        exact.observe_chunk(None, true);
        let exact_line = exact.take_summary_line().unwrap();
        assert!(exact_line.contains("outcome=success"));
        assert!(exact_line.contains("kind=message"));

        let mut over = json_owner();
        over.observe_chunk(Some(&Bytes::from(exact_body)), false);
        over.observe_chunk(Some(&Bytes::from_static(b"x")), true);
        let over_line = over.take_summary_line().unwrap();
        assert!(over_line.contains("reason=oversized-body"));
        assert!(over_line.contains("outcome=unknown"));
    }

    #[test]
    fn sse_events_use_canonical_decoder_and_first_terminal_state_wins() {
        let stream = Bytes::from_static(
            b"data: {\"jsonrpc\":\"2.0\",\"result\":{\"task\":{\"status\":{\"state\":\"TASK_STATE_WORKING\"}}}}\n\n\
              data: {\"jsonrpc\":\"2.0\",\"result\":{\"statusUpdate\":{\"status\":{\"state\":\"TASK_STATE_COMPLETED\"}}}}\n\n\
              data: {\"jsonrpc\":\"2.0\",\"result\":{\"statusUpdate\":{\"status\":{\"state\":\"TASK_STATE_FAILED\"}}}}\n\n",
        );
        for split in 0..=stream.len() {
            let mut owner = sse_owner();
            owner.observe_chunk(Some(&stream.slice(..split)), false);
            owner.observe_chunk(Some(&stream.slice(split..)), true);
            let line = owner.take_summary_line().unwrap();
            assert!(line.contains("outcome=success"), "{split}: {line}");
            assert!(line.contains("kind=status-update"), "{split}: {line}");
            assert!(line.contains("state=completed"), "{split}: {line}");
            assert!(line.contains("events=3"), "{split}: {line}");
        }
    }

    #[test]
    fn sse_kinds_errors_and_multiple_events_are_bounded_enums() {
        let stream = Bytes::from_static(
            b"id: private\ndata: {\"jsonrpc\":\"2.0\",\"result\":{\"artifactUpdate\":{\"artifactId\":\"secret\"}}}\n\n\
              data: {\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32042,\"message\":\"hidden\"}}\n\n",
        );
        let mut owner = sse_owner();
        owner.observe_chunk(Some(&stream), true);
        let line = owner.take_summary_line().unwrap();
        assert!(line.contains("outcome=error"));
        assert!(line.contains("error=a2a"));
        assert!(line.contains("events=2"));
        assert!(!line.contains("private"));
        assert!(!line.contains("secret"));
        assert!(!line.contains("hidden"));
    }

    #[test]
    fn sse_event_after_jsonrpc_error_is_unknown() {
        let stream = Bytes::from_static(
            b"data: {\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32042}}\n\n\
              data: {\"jsonrpc\":\"2.0\",\"result\":{\"message\":{}}}\n\n",
        );
        let mut owner = sse_owner();
        owner.observe_chunk(Some(&stream), true);
        let line = owner.take_summary_line().unwrap();
        assert!(line.contains("outcome=unknown"), "{line}");
        assert!(line.contains("reason=event-after-error"), "{line}");
    }

    #[test]
    fn oversized_sse_event_degrades_to_unknown_without_changing_source_bytes() {
        let mut stream = Vec::with_capacity(MAX_JSON_RESPONSE_BYTES + 16);
        stream.extend_from_slice(b"data: ");
        stream.extend(std::iter::repeat_n(b'x', MAX_JSON_RESPONSE_BYTES + 1));
        stream.extend_from_slice(b"\n\n");
        let stream = Bytes::from(stream);
        let before = stream.clone();
        let before_ptr = stream.as_ptr();

        let mut owner = sse_owner();
        owner.observe_chunk(Some(&stream), true);
        let line = owner.take_summary_line().unwrap();

        assert!(line.contains("outcome=unknown"), "{line}");
        assert!(line.contains("reason=oversized-event"), "{line}");
        assert_eq!(stream, before);
        assert_eq!(stream.as_ptr(), before_ptr);
    }

    #[test]
    fn long_sse_stream_and_aggregate_counters_remain_bounded() {
        const EVENT_COUNT: usize = 10_000;
        const EVENT: &[u8] = b"data: {\"jsonrpc\":\"2.0\",\"result\":{\"message\":{}}}\n\n";
        let mut stream = Vec::with_capacity(EVENT.len() * EVENT_COUNT);
        for _ in 0..EVENT_COUNT {
            stream.extend_from_slice(EVENT);
        }

        let mut owner = sse_owner();
        owner.observe_chunk(Some(&Bytes::from(stream)), true);
        let line = owner.take_summary_line().unwrap();
        assert!(line.contains("outcome=success"), "{line}");
        assert!(line.contains("events=10000"), "{line}");

        let envelope =
            serde_json::from_slice::<JsonRpcEnvelope>(br#"{"jsonrpc":"2.0","result":{"message":{}}}"#).unwrap();
        let mut aggregate = Aggregate {
            events: u32::MAX,
            malformed: u32::MAX,
            ..Aggregate::default()
        };
        aggregate.observe_envelope(envelope);
        aggregate.mark_unknown("malformed-event");
        assert_eq!(aggregate.events, u32::MAX);
        assert_eq!(aggregate.malformed, u32::MAX);
    }

    #[test]
    fn every_a2a_task_state_maps_to_a_bounded_enum() {
        for (wire, expected, summary, terminal) in [
            ("TASK_STATE_UNSPECIFIED", TaskState::Unspecified, "unspecified", false),
            ("TASK_STATE_SUBMITTED", TaskState::Submitted, "submitted", false),
            ("TASK_STATE_WORKING", TaskState::Working, "working", false),
            ("TASK_STATE_COMPLETED", TaskState::Completed, "completed", true),
            ("TASK_STATE_FAILED", TaskState::Failed, "failed", true),
            ("TASK_STATE_CANCELED", TaskState::Canceled, "canceled", true),
            (
                "TASK_STATE_INPUT_REQUIRED",
                TaskState::InputRequired,
                "input-required",
                false,
            ),
            ("TASK_STATE_REJECTED", TaskState::Rejected, "rejected", true),
            (
                "TASK_STATE_AUTH_REQUIRED",
                TaskState::AuthRequired,
                "auth-required",
                false,
            ),
        ] {
            let state = serde_json::from_str::<TaskState>(&format!("\"{wire}\"")).unwrap();
            assert_eq!(state, expected);
            assert_eq!(state.as_str(), summary);
            assert_eq!(state.is_terminal(), terminal);
        }

        let unknown = serde_json::from_str::<TaskState>("\"TASK_STATE_FUTURE\"").unwrap();
        assert_eq!(unknown, TaskState::Unknown);
        assert_eq!(unknown.as_str(), "unknown");
        assert!(!unknown.is_terminal());
    }

    #[test]
    fn request_owner_generations_are_isolated_across_hot_reload_and_recreate() {
        let mut old_generation = json_owner();
        let old_body =
            Bytes::from_static(br#"{"jsonrpc":"2.0","result":{"task":{"status":{"state":"TASK_STATE_WORKING"}}}}"#);
        let old_split = old_body.len() / 2;
        old_generation.observe_chunk(Some(&old_body.slice(..old_split)), false);

        let mut new_generation = sse_owner();
        new_generation.observe_chunk(
            Some(&Bytes::from_static(
                b"data: {\"jsonrpc\":\"2.0\",\"result\":{\"statusUpdate\":{\"status\":{\"state\":\"TASK_STATE_COMPLETED\"}}}}\n\n",
            )),
            true,
        );

        old_generation.observe_chunk(Some(&old_body.slice(old_split..)), true);
        let new_line = new_generation.take_summary_line().unwrap();
        let old_line = old_generation.take_summary_line().unwrap();
        assert!(new_line.contains("kind=status-update"), "{new_line}");
        assert!(new_line.contains("state=completed"), "{new_line}");
        assert!(old_line.contains("kind=task"), "{old_line}");
        assert!(old_line.contains("state=working"), "{old_line}");

        let mut deleted_generation = sse_owner();
        deleted_generation.observe_chunk(Some(&Bytes::from_static(b"data: partial")), false);
        drop(deleted_generation);

        let mut recreated_generation = json_owner();
        recreated_generation.observe_chunk(
            Some(&Bytes::from_static(br#"{"jsonrpc":"2.0","error":{"code":-32042}}"#)),
            true,
        );
        let recreated_line = recreated_generation.take_summary_line().unwrap();
        assert!(recreated_line.contains("outcome=error"), "{recreated_line}");
        assert!(recreated_line.contains("error=a2a"), "{recreated_line}");
        assert!(recreated_line.contains("events=1"), "{recreated_line}");
    }

    #[test]
    fn malformed_and_partial_sse_are_unknown_without_retaining_payload() {
        for stream in [
            Bytes::from_static(b"data: not-json\n\n"),
            Bytes::from_static(b"data: {\"jsonrpc\":\"2.0\",\"result\":{}"),
        ] {
            let mut owner = sse_owner();
            owner.observe_chunk(Some(&stream), true);
            let line = owner.take_summary_line().unwrap();
            assert!(line.contains("outcome=unknown"));
            assert!(line.contains("malformed=1"));
            assert!(!line.contains("not-json"));
        }
    }

    #[test]
    fn observation_does_not_change_or_retain_source_bytes() {
        let body = Bytes::from_static(br#"{"jsonrpc":"2.0","result":{"message":{}}}"#);
        let before_ptr = body.as_ptr();
        let before = body.clone();
        let mut owner = sse_owner();
        owner.observe_chunk(
            Some(&Bytes::from(format!("data: {}\n\n", String::from_utf8_lossy(&body)))),
            true,
        );
        assert_eq!(body, before);
        assert_eq!(body.as_ptr(), before_ptr);
    }

    #[test]
    fn incomplete_and_summary_drain_are_exactly_once() {
        let mut owner = json_owner();
        owner.observe_chunk(Some(&Bytes::from_static(b"{")), false);
        owner.terminalize_incomplete("unknown", "client-cancel");
        owner.terminalize_incomplete("error", "later");
        let line = owner.take_summary_line().unwrap();
        assert!(line.contains("outcome=unknown"));
        assert!(line.contains("reason=client-cancel"));
        assert!(owner.take_summary_line().is_none());
    }
}
