// Transport event recorder.
//
// The two raw transport backends (`transport::h1`, `transport::h2`) write every
// wire-level fact they observe into one process-global log. Integration cases
// read it back over the ordinary axum router at `/transport/query/{trace_id}`,
// exactly the way the RequestMirror cases read `/mirror/query/{trace_id}`.
//
// Why a recorder and not a set of echo headers: every interesting claim in this
// area ("the connection was not reused", "the gateway reset the stream", "the
// upstream never saw a body") is a claim about what the gateway put on the wire
// toward the upstream. An echoed response header only proves what the backend
// *application* saw, and it does not exist at all on the paths where the
// response is never written.
//
// Correlation rule (load-bearing): `ConnAccepted` / `ConnClosed` happen before
// any trace id is known, so a query for trace `t` returns
//   (a) every event carrying `trace_id == t`,
//   (b) every *untagged* event recorded on a connection that ever carried `t`,
//   (c) a `connections[]` summary for those connections, and
//   (d) `conn_accepted_total`, a global monotonic per-listener counter.
// Without (b) and (d), connection reuse cannot be asserted at all: a bound like
// `connections <= 1` is also satisfied by zero connections.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

/// Listener label for the raw HTTP/1.1 transport backend.
pub const LISTENER_H1: &str = "h1";
/// Listener label for the raw HTTP/2 (h2c prior-knowledge) transport backend.
pub const LISTENER_H2: &str = "h2";

/// A connection was accepted. Recorded at `accept()`, never at first request —
/// that is the whole point, because pool reuse is decided before any request.
pub const KIND_CONN_ACCEPTED: &str = "ConnAccepted";
/// Request headers were fully received.
pub const KIND_REQ_HEADERS: &str = "ReqHeaders";
/// A request body chunk was received. Exactly one carries `end_stream: true`
/// per completed request body, including the empty ones.
pub const KIND_REQ_DATA: &str = "ReqData";
/// A retry-probe script committed one simulated application operation. The
/// length and digest describe its complete input, not transport acceptance.
pub const KIND_EFFECT_APPLIED: &str = "EffectApplied";
/// Real trailer fields were observed (an H1 `Trailer:` declaration alone, or a
/// bare `0\r\n\r\n`, does not count).
pub const KIND_REQ_TRAILERS: &str = "ReqTrailers";
/// The peer (i.e. the gateway) reset the stream or the connection.
pub const KIND_PEER_RESET: &str = "PeerReset";
/// The peer sent GOAWAY.
pub const KIND_PEER_GOAWAY: &str = "PeerGoaway";
/// The peer half-closed (FIN) while the backend was still reading.
pub const KIND_PEER_HALF_CLOSE: &str = "PeerHalfClose";
/// The backend wrote a response head. Carries what was actually emitted.
pub const KIND_RESP_HEAD: &str = "RespHead";
/// The backend finished writing the response body.
pub const KIND_RESP_DONE: &str = "RespDone";
/// The backend's connection task exited.
pub const KIND_CONN_CLOSED: &str = "ConnClosed";

/// Hard cap on retained events. The log is process-global and every case calls
/// `/transport/reset` first, so this only guards against a runaway backend.
const MAX_EVENTS: usize = 20_000;

/// One observed transport fact.
///
/// Per-kind fields are `Option` and omitted from the JSON when absent, so a
/// consumer can match on `kind` and read only the fields that kind populates.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransportEvent {
    /// Global monotonic ordinal, assigned at record time.
    pub seq: u64,
    /// Millis since the recorder's origin (reset by `/transport/reset`). The
    /// single monotonic anchor every ordering claim is expressed against.
    pub at_ms: u64,
    /// `"h1"` or `"h2"`.
    pub listener: String,
    /// Connection ordinal within the listener, claimed at `accept()`.
    pub conn: u32,
    /// H1: request ordinal on the connection (1-based). H2: stream id.
    pub stream: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Trace-scoped request ordinal for behaviors whose result changes between
    /// attempts carrying the same trace id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_request_ordinal: Option<u32>,
    pub kind: String,
    /// `ReqHeaders`. H2: END_STREAM on HEADERS. H1: neither CL nor TE present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers_eos: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `ReqHeaders`: request target observed on the upstream protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `ReqHeaders`: H1 Host or H2 URI authority observed by the upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// `ReqHeaders`: parsed `grpc-timeout`, split so the recorder never stores
    /// arbitrary request-header text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc_timeout_value: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc_timeout_unit: Option<String>,
    /// `ReqData`: bytes in this chunk. `ReqTrailers`: number of fields.
    /// `RespHead`/`RespDone`: response body bytes actually emitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_stream: Option<bool>,
    /// `PeerReset` / `PeerGoaway`: the H2 error code's canonical name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// `RespHead`: which `x-up-behavior` script produced this response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
    /// `RespHead`: the framing actually emitted, e.g. `content-length=16`,
    /// `chunked`, `content-length=16+chunked`, `chunked;size-line=24;actual=16`.
    /// This is how a malformed-framing case asserts what went on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<String>,
    /// Free-form note (unknown behavior name, backend-side degradation, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl TransportEvent {
    pub fn new(listener: &str, conn: u32, stream: u32, kind: &str) -> Self {
        Self {
            listener: listener.to_string(),
            conn,
            stream,
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    pub fn trace(mut self, trace_id: Option<&str>) -> Self {
        self.trace_id = trace_id.map(|t| t.to_string());
        self
    }

    pub fn trace_request_ordinal(mut self, ordinal: Option<u32>) -> Self {
        self.trace_request_ordinal = ordinal;
        self
    }

    pub fn headers_eos(mut self, v: bool) -> Self {
        self.headers_eos = Some(v);
        self
    }

    pub fn content_length(mut self, v: Option<u64>) -> Self {
        self.content_length = v;
        self
    }

    pub fn transfer_encoding(mut self, v: Option<String>) -> Self {
        self.transfer_encoding = v;
        self
    }

    pub fn method(mut self, v: &str) -> Self {
        self.method = Some(v.to_string());
        self
    }

    pub fn path(mut self, v: &str) -> Self {
        self.path = Some(v.to_string());
        self
    }

    pub fn target(mut self, v: &str) -> Self {
        self.target = Some(v.to_string());
        self
    }

    pub fn authority(mut self, v: &str) -> Self {
        self.authority = Some(v.to_string());
        self
    }

    pub fn grpc_timeout(mut self, value: u64, unit: char) -> Self {
        self.grpc_timeout_value = Some(value);
        self.grpc_timeout_unit = Some(unit.to_string());
        self
    }

    pub fn len(mut self, v: usize) -> Self {
        self.len = Some(v);
        self
    }

    pub fn end_stream(mut self, v: bool) -> Self {
        self.end_stream = Some(v);
        self
    }

    pub fn code(mut self, v: &str) -> Self {
        self.code = Some(v.to_string());
        self
    }

    pub fn status(mut self, v: u16) -> Self {
        self.status = Some(v);
        self
    }

    pub fn behavior(mut self, v: &str) -> Self {
        self.behavior = Some(v.to_string());
        self
    }

    pub fn framing(mut self, v: &str) -> Self {
        self.framing = Some(v.to_string());
        self
    }

    pub fn detail(mut self, v: &str) -> Self {
        self.detail = Some(v.to_string());
        self
    }
}

/// Per-connection summary returned by the query surface.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnSummary {
    pub listener: String,
    pub conn: u32,
    /// Requests this connection has served since it was accepted. Not cleared
    /// by `/transport/reset` — it belongs to the connection, not the case.
    pub requests: u32,
    /// Trace ids seen on this connection since the last `/transport/reset`.
    pub traces: Vec<String>,
    pub closed: bool,
}

/// The recorder state. Split out from the global so it can be unit-tested
/// without a shared static and without any network.
#[derive(Debug)]
pub struct TransportLog {
    seq: u64,
    origin: Instant,
    events: Vec<TransportEvent>,
    conns: Vec<ConnSummary>,
    /// Global monotonic per-listener accept counter. Deliberately NOT cleared
    /// by `reset()`: a case snapshots it before its request and asserts the
    /// delta, which is the only form of "no second connection was opened" that
    /// cannot be satisfied by an empty log.
    accepted_total: HashMap<String, u64>,
    next_conn: HashMap<String, u32>,
    trace_requests: HashMap<String, u32>,
    dropped: u64,
}

impl Default for TransportLog {
    fn default() -> Self {
        Self {
            seq: 0,
            origin: Instant::now(),
            events: Vec::new(),
            conns: Vec::new(),
            accepted_total: HashMap::new(),
            next_conn: HashMap::new(),
            trace_requests: HashMap::new(),
            dropped: 0,
        }
    }
}

impl TransportLog {
    fn conn_index(&self, listener: &str, conn: u32) -> Option<usize> {
        self.conns.iter().position(|c| c.listener == listener && c.conn == conn)
    }

    /// Claim a connection ordinal and record `ConnAccepted`.
    pub fn claim_conn(&mut self, listener: &str) -> u32 {
        let slot = self.next_conn.entry(listener.to_string()).or_insert(0);
        let conn = *slot;
        *slot += 1;
        *self.accepted_total.entry(listener.to_string()).or_insert(0) += 1;
        self.conns.push(ConnSummary {
            listener: listener.to_string(),
            conn,
            requests: 0,
            traces: Vec::new(),
            closed: false,
        });
        self.record(TransportEvent::new(listener, conn, 0, KIND_CONN_ACCEPTED));
        conn
    }

    /// Bump the connection's request counter and return the 1-based ordinal of
    /// the request that is starting.
    pub fn note_request(&mut self, listener: &str, conn: u32) -> u32 {
        match self.conn_index(listener, conn) {
            Some(i) => {
                self.conns[i].requests += 1;
                self.conns[i].requests
            }
            None => 1,
        }
    }

    /// Return the 1-based request ordinal for a trace in the current case.
    pub fn note_trace_request(&mut self, trace_id: &str) -> u32 {
        let slot = self.trace_requests.entry(trace_id.to_string()).or_insert(0);
        *slot += 1;
        *slot
    }

    /// Associate a trace id with a connection so connection-scoped events stay
    /// correlatable to the request that caused them.
    pub fn bind_trace(&mut self, listener: &str, conn: u32, trace_id: &str) {
        if let Some(i) = self.conn_index(listener, conn) {
            if !self.conns[i].traces.iter().any(|t| t == trace_id) {
                self.conns[i].traces.push(trace_id.to_string());
            }
        }
    }

    pub fn mark_closed(&mut self, listener: &str, conn: u32) {
        if let Some(i) = self.conn_index(listener, conn) {
            self.conns[i].closed = true;
        }
        self.record(TransportEvent::new(listener, conn, 0, KIND_CONN_CLOSED));
    }

    pub fn record(&mut self, mut ev: TransportEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.dropped += 1;
            return;
        }
        self.seq += 1;
        ev.seq = self.seq;
        ev.at_ms = self.origin.elapsed().as_millis() as u64;
        self.events.push(ev);
    }

    /// Clear the per-case state. Connection identity, the per-connection
    /// request counter and `accepted_total` survive, because live connections
    /// outlive a case and delta assertions depend on the counter being global.
    pub fn reset(&mut self) {
        self.seq = 0;
        self.origin = Instant::now();
        self.events.clear();
        self.trace_requests.clear();
        self.dropped = 0;
        for c in &mut self.conns {
            c.traces.clear();
        }
        self.conns.retain(|c| !c.closed);
    }

    /// Read accessors used by the unit tests. Over the wire the same facts are
    /// reached through `query()` / `dump()`.
    #[cfg(test)]
    pub fn accepted_total(&self) -> &HashMap<String, u64> {
        &self.accepted_total
    }

    #[cfg(test)]
    pub fn events(&self) -> &[TransportEvent] {
        &self.events
    }

    /// Apply the §2.5 correlation rule for one trace id.
    pub fn query(&self, trace_id: &str) -> serde_json::Value {
        // (c) every connection that ever carried this trace since the last reset.
        let carriers: Vec<&ConnSummary> = self
            .conns
            .iter()
            .filter(|c| c.traces.iter().any(|t| t == trace_id))
            .collect();

        let on_carrier = |ev: &TransportEvent| carriers.iter().any(|c| c.listener == ev.listener && c.conn == ev.conn);

        let events: Vec<TransportEvent> = self
            .events
            .iter()
            // (a) the trace's own events, plus (b) untagged events on a carrier.
            .filter(|ev| match ev.trace_id.as_deref() {
                Some(t) => t == trace_id,
                None => on_carrier(ev),
            })
            .cloned()
            .collect();

        let found = self.events.iter().any(|ev| ev.trace_id.as_deref() == Some(trace_id));

        serde_json::json!({
            "found": found,
            "count": events.len(),
            "events": events,
            "connections": carriers,
            "conn_accepted_total": self.accepted_total,
            "dropped": self.dropped,
        })
    }

    /// Everything, for a failure dump.
    pub fn dump(&self) -> serde_json::Value {
        serde_json::json!({
            "count": self.events.len(),
            "events": self.events,
            "connections": self.conns,
            "conn_accepted_total": self.accepted_total,
            "dropped": self.dropped,
        })
    }
}

static TRANSPORT_LOG: LazyLock<Mutex<TransportLog>> = LazyLock::new(|| Mutex::new(TransportLog::default()));

pub fn claim_conn(listener: &str) -> u32 {
    TRANSPORT_LOG.lock().claim_conn(listener)
}

pub fn note_request(listener: &str, conn: u32) -> u32 {
    TRANSPORT_LOG.lock().note_request(listener, conn)
}

pub fn note_trace_request(trace_id: &str) -> u32 {
    TRANSPORT_LOG.lock().note_trace_request(trace_id)
}

pub fn bind_trace(listener: &str, conn: u32, trace_id: &str) {
    TRANSPORT_LOG.lock().bind_trace(listener, conn, trace_id);
}

pub fn mark_closed(listener: &str, conn: u32) {
    TRANSPORT_LOG.lock().mark_closed(listener, conn);
}

pub fn record(ev: TransportEvent) {
    TRANSPORT_LOG.lock().record(ev);
}

/// The same correlated view `/transport/query/{trace_id}` serves, read in
/// process without the HTTP surface.
///
/// This exists for the frame-level proofs in `transport::h2::wire_proof`, whose
/// whole point is to compare what a backend *recorded* against what it actually
/// put on the wire. Routing that comparison through axum and `reqwest` would put
/// the thing under test behind two more layers.
#[cfg(test)]
pub fn query(trace_id: &str) -> serde_json::Value {
    TRANSPORT_LOG.lock().query(trace_id)
}

// ---------------------------------------------------------------------------
// Query surface — plain axum routes on the shared echo router, so a case reaches
// them with ordinary `reqwest`, exactly like `/mirror/query/{trace_id}`.
// ---------------------------------------------------------------------------

/// `GET /transport/query/{trace_id}`
pub async fn transport_query_handler(
    axum::extract::Path(trace_id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    let body = TRANSPORT_LOG.lock().query(&trace_id);
    (axum::http::StatusCode::OK, axum::response::Json(body))
}

/// `GET /transport/reset`
pub async fn transport_reset_handler() -> impl axum::response::IntoResponse {
    {
        let mut log = TRANSPORT_LOG.lock();
        log.reset();
    }
    (axum::http::StatusCode::OK, "transport-log-reset")
}

/// `GET /transport/dump`
pub async fn transport_dump_handler() -> impl axum::response::IntoResponse {
    let body = TRANSPORT_LOG.lock().dump();
    (axum::http::StatusCode::OK, axum::response::Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(v: &serde_json::Value) -> Vec<String> {
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn conn_ordinals_are_per_listener_and_monotonic() {
        let mut log = TransportLog::default();
        assert_eq!(log.claim_conn(LISTENER_H1), 0);
        assert_eq!(log.claim_conn(LISTENER_H1), 1);
        assert_eq!(log.claim_conn(LISTENER_H2), 0);
        assert_eq!(log.accepted_total().get(LISTENER_H1), Some(&2));
        assert_eq!(log.accepted_total().get(LISTENER_H2), Some(&1));
    }

    #[test]
    fn query_returns_trace_events_plus_connection_scoped_events() {
        let mut log = TransportLog::default();
        let conn = log.claim_conn(LISTENER_H1);
        log.bind_trace(LISTENER_H1, conn, "t-a");
        log.record(
            TransportEvent::new(LISTENER_H1, conn, 1, KIND_REQ_HEADERS)
                .trace(Some("t-a"))
                .method("POST")
                .path("/x"),
        );
        log.record(
            TransportEvent::new(LISTENER_H1, conn, 1, KIND_REQ_DATA)
                .trace(Some("t-a"))
                .len(4)
                .end_stream(true),
        );
        log.mark_closed(LISTENER_H1, conn);

        let q = log.query("t-a");
        assert_eq!(q["found"], true);
        // ConnAccepted and ConnClosed carry no trace id, yet must be returned:
        // they are the connection-scoped half of the correlation rule.
        assert_eq!(
            kinds(&q),
            vec![KIND_CONN_ACCEPTED, KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_CONN_CLOSED]
        );
        assert_eq!(q["count"], 4);
        assert_eq!(q["connections"].as_array().unwrap().len(), 1);
        assert_eq!(q["connections"][0]["traces"][0], "t-a");
        assert_eq!(q["connections"][0]["closed"], true);
        assert_eq!(q["conn_accepted_total"]["h1"], 1);
    }

    #[test]
    fn query_excludes_other_connections_and_other_traces() {
        let mut log = TransportLog::default();
        let a = log.claim_conn(LISTENER_H1);
        let b = log.claim_conn(LISTENER_H1);
        log.bind_trace(LISTENER_H1, a, "t-a");
        log.bind_trace(LISTENER_H1, b, "t-b");
        log.record(TransportEvent::new(LISTENER_H1, a, 1, KIND_REQ_HEADERS).trace(Some("t-a")));
        log.record(TransportEvent::new(LISTENER_H1, b, 1, KIND_REQ_HEADERS).trace(Some("t-b")));

        let q = log.query("t-a");
        // conn 1's ConnAccepted must NOT leak into conn 0's trace query.
        assert_eq!(q["count"], 2);
        for ev in q["events"].as_array().unwrap() {
            assert_eq!(ev["conn"], 0);
        }
        // ... but the global accept counter still reports both, which is what a
        // "no second connection was opened" delta assertion reads.
        assert_eq!(q["conn_accepted_total"]["h1"], 2);
    }

    #[test]
    fn reuse_of_one_connection_shows_both_traces() {
        let mut log = TransportLog::default();
        let conn = log.claim_conn(LISTENER_H1);
        assert_eq!(log.note_request(LISTENER_H1, conn), 1);
        log.bind_trace(LISTENER_H1, conn, "t-a");
        assert_eq!(log.note_request(LISTENER_H1, conn), 2);
        log.bind_trace(LISTENER_H1, conn, "t-b");
        log.record(TransportEvent::new(LISTENER_H1, conn, 2, KIND_REQ_HEADERS).trace(Some("t-b")));

        let q = log.query("t-b");
        assert_eq!(q["connections"][0]["requests"], 2);
        assert_eq!(q["connections"][0]["traces"][0], "t-a");
        assert_eq!(q["connections"][0]["traces"][1], "t-b");
        assert_eq!(q["conn_accepted_total"]["h1"], 1);
    }

    #[test]
    fn unknown_trace_reports_not_found_without_events() {
        let log = TransportLog::default();
        let q = log.query("nope");
        assert_eq!(q["found"], false);
        assert_eq!(q["count"], 0);
        assert!(q["events"].as_array().unwrap().is_empty());
        assert!(q["connections"].as_array().unwrap().is_empty());
        assert_eq!(q["conn_accepted_total"], serde_json::json!({}));
    }

    #[test]
    fn reset_clears_events_and_traces_but_keeps_the_global_accept_counter() {
        let mut log = TransportLog::default();
        let conn = log.claim_conn(LISTENER_H1);
        log.bind_trace(LISTENER_H1, conn, "t-a");
        log.record(TransportEvent::new(LISTENER_H1, conn, 1, KIND_REQ_HEADERS).trace(Some("t-a")));

        log.reset();

        assert!(log.events().is_empty());
        assert_eq!(log.query("t-a")["found"], false);
        // Still one live connection, still one lifetime accept.
        assert_eq!(log.accepted_total().get(LISTENER_H1), Some(&1));
        log.bind_trace(LISTENER_H1, conn, "t-b");
        assert_eq!(log.query("t-b")["connections"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn reset_forgets_connections_that_already_closed() {
        let mut log = TransportLog::default();
        let conn = log.claim_conn(LISTENER_H1);
        log.bind_trace(LISTENER_H1, conn, "t-a");
        log.mark_closed(LISTENER_H1, conn);
        log.reset();
        assert!(log.dump()["connections"].as_array().unwrap().is_empty());
        assert_eq!(log.accepted_total().get(LISTENER_H1), Some(&1));
    }

    #[test]
    fn seq_is_monotonic_and_serialization_omits_absent_fields() {
        let mut log = TransportLog::default();
        log.record(TransportEvent::new(LISTENER_H2, 0, 1, KIND_PEER_RESET).code("CANCEL"));
        log.record(TransportEvent::new(LISTENER_H2, 0, 1, KIND_CONN_CLOSED));
        let evs = log.events();
        assert_eq!(evs[0].seq, 1);
        assert_eq!(evs[1].seq, 2);

        let json = serde_json::to_value(&evs[0]).unwrap();
        assert_eq!(json["code"], "CANCEL");
        assert!(json.get("status").is_none());
        assert!(json.get("trace_id").is_none());
        // The shape round-trips, so the client-side probe can deserialize it.
        let back: TransportEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, evs[0]);
    }
}
