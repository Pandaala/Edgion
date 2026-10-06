//! Awaited JSON inspection through the canonical request-body capture seam.
use super::*;
use crate::plugins::runtime::traits::session::RequestBodyChunkConsumer;
use bytes::Bytes;
use edgion_waf::seclang::JsonInspectionSession;

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
    inspection: JsonInspectionSession<'call, 'generation>,
    charge: RuntimeCharge,
    inspected: usize,
    failure: Option<RuntimeError>,
}
impl JsonCapture<'_, '_> {
    fn refresh(&mut self) {
        let usage = self.inspection.usage();
        let delta = usage.inspected_source_bytes.saturating_sub(self.inspected);
        if delta != 0 {
            crate::observe::metrics::record_waf_body_bytes_inspected("request", delta);
        }
        self.inspected = usage.inspected_source_bytes;
        if let Some(bytes) = self.inspection.runtime_bytes() {
            self.charge.update(bytes);
        } else {
            self.failure.get_or_insert(RuntimeError::Arithmetic);
        }
    }
}
impl RequestBodyChunkConsumer for JsonCapture<'_, '_> {
    fn consume(&mut self, chunk: &[u8]) {
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
    limits: JsonInspectionLimits,
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
    if len > limits.lexical.max_source_bytes {
        return Err(JsonCaptureError::Runtime(RuntimeError::JsonChecker {
            kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                kind: JsonStreamLimit::SourceBytes,
                limit: limits.lexical.max_source_bytes,
            }),
        }));
    }
    Ok(len)
}

pub(super) async fn inspect(
    session: &mut dyn PluginSession,
    transaction: &mut Transaction<'static>,
    limits: JsonInspectionLimits,
    runtime: RuntimeLimits,
    capture_cap: u64,
    requires_body: bool,
) -> Result<(), JsonCaptureError> {
    // Cached size admission precedes reader IO, matching the complete adapter.
    let hint = if requires_body {
        session
            .ctx()
            .request_body
            .cached_len()
            .map_err(JsonCaptureError::Body)?
            .map(|len| admitted_len(len, runtime, limits, capture_cap))
            .transpose()?
    } else {
        None
    };
    let inspection = transaction
        .start_json_inspection(limits, hint)
        .map_err(JsonCaptureError::Runtime)?;
    let mut capture = JsonCapture {
        inspection,
        charge: RuntimeCharge { bytes: 0 },
        inspected: 0,
        failure: None,
    };
    capture.refresh();
    let captured = if requires_body {
        session
            .capture_request_body_with_consumer(Some(capture_cap), &mut capture)
            .await
            .map_err(JsonCaptureError::Body)?
    } else {
        0
    };
    let known_bodyless = session.ctx().request_info.body_declared == Some(false);
    let available = if requires_body {
        session
            .request_body_available_len(known_bodyless)
            .map_err(JsonCaptureError::Body)?
    } else {
        0
    };
    if available != captured {
        return Err(JsonCaptureError::Body(BodyReadError::Incomplete));
    }
    let len = admitted_len(captured, runtime, limits, capture_cap)?;
    let completed = capture.inspection.complete(len);
    capture.refresh();
    // An actually required canonical raw read keeps its storage-error priority.
    // JSON-only rules never materialize the captured source.
    let source = if capture.inspection.requires_raw_source() && !requires_body {
        Some(Bytes::new())
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
    inspection.finish(source).map_err(JsonCaptureError::Runtime)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/waf/json_capture_test.rs"]
mod tests;
