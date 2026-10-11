//! Awaited structured inspection through the canonical request-body capture seam.
use super::*;
use crate::plugins::runtime::traits::session::RequestBodyChunkConsumer;
use bytes::Bytes;
use edgion_waf::seclang::{
    JsonInspectionSession, XmlInspectionLimits, XmlInspectionSession, XmlStreamError, XmlStreamLimit,
};

// Bound synchronous cached input; larger snapshots keep the ordinary reader path.
const CACHED_JSON_CONTIGUOUS_LIMIT: usize = 128 * 1024;

#[derive(Clone, Copy)]
enum InspectionLimits {
    Json(JsonInspectionLimits),
    Xml(XmlInspectionLimits),
}
impl InspectionLimits {
    fn source_limit(self) -> usize {
        match self {
            Self::Json(limits) => limits.lexical.max_source_bytes,
            Self::Xml(limits) => limits.parser.max_source_bytes,
        }
    }
    fn source_error(self) -> RuntimeError {
        match self {
            Self::Json(limits) => RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                    kind: JsonStreamLimit::SourceBytes,
                    limit: limits.lexical.max_source_bytes,
                }),
            },
            Self::Xml(limits) => RuntimeError::XmlStream {
                kind: XmlStreamError::Limit {
                    kind: XmlStreamLimit::SourceBytes,
                    limit: limits.parser.max_source_bytes,
                },
            },
        }
    }
}
// Keep both borrowed sessions inline to avoid a request-path heap allocation.
#[allow(clippy::large_enum_variant)]
enum InspectionSession<'call, 'generation> {
    Json(JsonInspectionSession<'call, 'generation>),
    Xml(XmlInspectionSession<'call, 'generation>),
}
impl InspectionSession<'_, '_> {
    fn feed(&mut self, data: &[u8]) -> Result<(), RuntimeError> {
        match self {
            Self::Json(session) => session.feed(data),
            Self::Xml(session) => session.feed(data),
        }
    }
    fn complete(&mut self, len: usize) -> Result<(), RuntimeError> {
        match self {
            Self::Json(session) => session.complete(len),
            Self::Xml(session) => session.complete(len),
        }
    }
    fn requires_raw_source(&self) -> bool {
        match self {
            Self::Json(session) => session.requires_raw_source(),
            Self::Xml(session) => session.requires_raw_source(),
        }
    }
    fn runtime_bytes(&self) -> Option<usize> {
        match self {
            Self::Json(session) => session.runtime_bytes(),
            Self::Xml(session) => session.runtime_bytes(),
        }
    }
    fn inspected_source_bytes(&self) -> usize {
        match self {
            Self::Json(session) => session.usage().inspected_source_bytes,
            Self::Xml(session) => session.usage().parser.source_bytes,
        }
    }
    fn stop_by_policy(self) -> Result<(), RuntimeError> {
        match self {
            Self::Json(session) => session.stop_by_policy(),
            Self::Xml(session) => session.stop_by_policy(),
        }
    }
    fn finish(self, source: Option<Bytes>) -> Result<(), RuntimeError> {
        match self {
            Self::Json(session) => session.finish(source),
            Self::Xml(session) => session.finish(source),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct InspectionPolicy {
    pub cap: u64,
    pub window: u64,
    pub native_prefix: bool,
    pub storage_limit: bool,
}
impl InspectionPolicy {
    pub fn complete(cap: u64) -> Self {
        Self {
            cap,
            window: cap,
            native_prefix: false,
            storage_limit: false,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InspectionOutcome {
    Complete,
    Limited,
}

pub(super) enum JsonCaptureError {
    Body(BodyReadError),
    Runtime(RuntimeError),
}

/// Temporarily accounts the transaction's logical state plus known provisional
/// allocation capacities. This is not allocator overhead, source storage or RSS.
struct RuntimeCharge {
    bytes: i64,
}
impl RuntimeCharge {
    fn update(&mut self, bytes: usize) {
        let next = i64::try_from(bytes).unwrap_or(i64::MAX);
        let delta = next.saturating_sub(self.bytes);
        if delta != 0 {
            crate::observe::metrics::adjust_waf_runtime_bytes_active(delta);
        }
        self.bytes = next;
    }
}
impl Drop for RuntimeCharge {
    fn drop(&mut self) {
        if self.bytes != 0 {
            crate::observe::metrics::adjust_waf_runtime_bytes_active(-self.bytes);
        }
    }
}

struct JsonCapture<'call, 'generation> {
    inspection: InspectionSession<'call, 'generation>,
    charge: RuntimeCharge,
    inspected: usize,
    failure: Option<RuntimeError>,
    window: u64,
    supplied: u64,
}
impl JsonCapture<'_, '_> {
    fn refresh(&mut self) {
        let inspected = self.inspection.inspected_source_bytes();
        let delta = inspected.saturating_sub(self.inspected);
        if delta != 0 {
            crate::observe::metrics::record_waf_body_bytes_inspected("request", delta);
        }
        self.inspected = inspected;
        if let Some(bytes) = self.inspection.runtime_bytes() {
            self.charge.update(bytes);
        } else {
            self.failure.get_or_insert(RuntimeError::Arithmetic);
        }
    }
}
impl RequestBodyChunkConsumer for JsonCapture<'_, '_> {
    fn source_window(&self) -> Option<u64> {
        Some(self.window)
    }
    fn cached_contiguous_limit(&self) -> Option<usize> {
        match self.inspection {
            InspectionSession::Json(_) => Some(CACHED_JSON_CONTIGUOUS_LIMIT),
            InspectionSession::Xml(_) => None,
        }
    }

    fn consume(&mut self, chunk: &[u8]) {
        let len = self.window.saturating_sub(self.supplied).min(chunk.len() as u64) as usize;
        let chunk = &chunk[..len];
        self.supplied += len as u64;
        if chunk.is_empty() {
            return;
        }
        if self.failure.is_none() {
            if let Err(error) = self.inspection.feed(chunk) {
                self.failure = Some(error);
            }
        }
        self.refresh();
    }
}

fn admitted_len(
    len: u64,
    runtime: RuntimeLimits,
    limits: InspectionLimits,
    cap: u64,
) -> Result<usize, JsonCaptureError> {
    if len > cap {
        return Err(JsonCaptureError::Body(BodyReadError::BufferLimitExceeded {
            limit: cap,
        }));
    }
    let len = usize::try_from(len).map_err(|_| JsonCaptureError::Runtime(RuntimeError::Arithmetic))?;
    if len > runtime.body.max_body_bytes {
        return Err(JsonCaptureError::Runtime(RuntimeError::Body {
            kind: BodyError::ByteLimit {
                actual: len,
                limit: runtime.body.max_body_bytes,
            },
        }));
    }
    if len > limits.source_limit() {
        return Err(JsonCaptureError::Runtime(limits.source_error()));
    }
    Ok(len)
}

pub(super) async fn inspect(
    session: &mut dyn PluginSession,
    transaction: &mut Transaction<'static>,
    limits: JsonInspectionLimits,
    runtime: RuntimeLimits,
    policy: InspectionPolicy,
    requires_body: bool,
) -> Result<InspectionOutcome, JsonCaptureError> {
    inspect_structured(
        session,
        transaction,
        InspectionLimits::Json(limits),
        runtime,
        policy,
        requires_body,
    )
    .await
}
pub(super) async fn inspect_xml(
    session: &mut dyn PluginSession,
    transaction: &mut Transaction<'static>,
    limits: XmlInspectionLimits,
    runtime: RuntimeLimits,
    policy: InspectionPolicy,
    requires_body: bool,
) -> Result<InspectionOutcome, JsonCaptureError> {
    inspect_structured(
        session,
        transaction,
        InspectionLimits::Xml(limits),
        runtime,
        policy,
        requires_body,
    )
    .await
}
async fn inspect_structured(
    session: &mut dyn PluginSession,
    transaction: &mut Transaction<'static>,
    limits: InspectionLimits,
    runtime: RuntimeLimits,
    policy: InspectionPolicy,
    requires_body: bool,
) -> Result<InspectionOutcome, JsonCaptureError> {
    let capture_cap = policy.cap;
    // Cached size admission precedes reader IO, matching the complete adapter.
    let hint = if requires_body {
        session
            .ctx()
            .request_body
            .cached_len()
            .map_err(JsonCaptureError::Body)?
            .map(|len| {
                if len > capture_cap {
                    return Err(JsonCaptureError::Body(BodyReadError::BufferLimitExceeded {
                        limit: capture_cap,
                    }));
                }
                admitted_len(len.min(policy.window), runtime, limits, capture_cap)
            })
            .transpose()?
    } else {
        None
    };
    let inspection = match limits {
        InspectionLimits::Json(limits) => InspectionSession::Json(
            transaction
                .start_json_inspection(limits, hint)
                .map_err(JsonCaptureError::Runtime)?,
        ),
        InspectionLimits::Xml(limits) => InspectionSession::Xml(
            transaction
                .start_xml_inspection(limits, hint)
                .map_err(JsonCaptureError::Runtime)?,
        ),
    };
    let mut capture = JsonCapture {
        inspection,
        charge: RuntimeCharge { bytes: 0 },
        inspected: 0,
        failure: None,
        window: policy.window,
        supplied: 0,
    };
    capture.refresh();
    let mut native_source = None;
    let mut native_complete = true;
    let captured = if requires_body && policy.native_prefix {
        let result = session
            .capture_request_body_inspection(policy.window, &mut capture)
            .await
            .map_err(JsonCaptureError::Body)?;
        native_complete = result.complete;
        native_source = result.storage;
        result.captured_len
    } else if requires_body {
        session
            .capture_request_body_with_consumer(Some(capture_cap), &mut capture)
            .await
            .map_err(JsonCaptureError::Body)?
    } else {
        0
    };
    let known_bodyless = session.ctx().request_info.body_declared == Some(false);
    let available = if requires_body && !policy.native_prefix {
        session
            .request_body_available_len(known_bodyless)
            .map_err(JsonCaptureError::Body)?
    } else {
        captured
    };
    if available != captured {
        return Err(JsonCaptureError::Body(BodyReadError::Incomplete));
    }
    if captured > capture_cap {
        return Err(JsonCaptureError::Body(BodyReadError::BufferLimitExceeded {
            limit: capture_cap,
        }));
    }
    if captured > policy.window || !native_complete {
        if let Some(error) = capture.failure {
            return Err(JsonCaptureError::Runtime(error));
        }
        let JsonCapture { inspection, charge, .. } = capture;
        drop(charge);
        inspection.stop_by_policy().map_err(JsonCaptureError::Runtime)?;
        return Ok(InspectionOutcome::Limited);
    }
    let len = admitted_len(captured, runtime, limits, capture_cap)?;
    let completed = capture.inspection.complete(len);
    capture.refresh();
    // An actually required canonical raw read keeps its storage-error priority.
    // JSON-only rules never materialize the captured source.
    let source = if capture.inspection.requires_raw_source() && !requires_body {
        Some(Bytes::new())
    } else if capture.inspection.requires_raw_source() && policy.native_prefix {
        let source = match native_source {
            Some(view) => view.read_all().await.map_err(JsonCaptureError::Body)?,
            None if len == 0 => Bytes::new(),
            None => return Err(JsonCaptureError::Body(BodyReadError::Incomplete)),
        };
        if source.len() != len {
            return Err(JsonCaptureError::Body(BodyReadError::Incomplete));
        }
        Some(source)
    } else if capture.inspection.requires_raw_source() {
        let body = session
            .read_request_body(Some(capture_cap))
            .await
            .map_err(JsonCaptureError::Body)?;
        if body.len() != len {
            return Err(JsonCaptureError::Body(BodyReadError::Incomplete));
        }
        Some(body)
    } else {
        None::<Bytes>
    };
    completed.map_err(JsonCaptureError::Runtime)?;
    if let Some(error) = capture.failure {
        return Err(JsonCaptureError::Runtime(error));
    }
    let JsonCapture { inspection, charge, .. } = capture;
    drop(charge);
    inspection.finish(source).map_err(JsonCaptureError::Runtime)?;
    Ok(InspectionOutcome::Complete)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/waf/json_capture_test.rs"]
mod tests;
