//! Transport-level request-body shapes, driven through the real gateway.
//!
//! # What this suite is for
//!
//! The fork's request-body seam decides framing, termination hygiene, benign
//! reset classification and H1 pool poisoning. `pingora-proxy`'s own
//! `test_request_body_seam.rs` proves what pingora does *when the hook says X*
//! against a stub. This suite proves what happens when the hook is driven by a
//! real `EdgionPlugins` chain, a real Gateway API route, a real upstream
//! connection pool and Edgion's own `upstream_request_filter` header rewrite —
//! none of which a fork test can see.
//!
//! The division of labour is stated once in
//! `skills/05-testing/01-integration-testing.md`; the short form is: *a claim
//! that can be made against a `SeamProxy` stub belongs in the fork; a claim that
//! needs a `Gateway` + `HTTPRoute` + `EdgionPlugins` belongs here.*
//!
//! # How a claim is made
//!
//! Every claim about what the gateway put on the wire toward the upstream goes
//! through the transport recorder (`/transport/query/{trace_id}` on the test
//! server), never through an echoed response header — on the paths that matter
//! no response is ever written, so an echoed header cannot exist. See
//! `crate::transport` for the client side and
//! `edgion-tests/src/server/transport/recorder.rs` for the recorder.
//!
//! # Anti-vacuity
//!
//! Three mechanisms, none of which depends on a contributor remembering them:
//!
//! 1. **Exercised-upstream guard.** [`run_transport_case`] fails the case unless
//!    the recorder saw a `ReqHeaders` for the case's trace, whatever the
//!    downstream status was. A case that never reached the upstream cannot pass.
//! 2. **Declared-kind guard.** Each shape declares the event kinds its
//!    assertions rest on; the wrapper verifies every declared kind is actually
//!    present. A shape named for a reset whose upstream never recorded one
//!    fails.
//! 3. **Negative claims are windows plus exact counts.** `expect_none` takes a
//!    cursor at call time, so it cannot pass by virtue of the log being empty
//!    before the case ran; it is always paired with an exact whole-log
//!    `count_matching(..) == n`, never a `<= n` bound that zero also satisfies.
//!
//! A combination a shape cannot express is emitted as an explicit
//! `..._skipped` case carrying its reason, so it appears in the report instead
//! of quietly shrinking the matrix. The `EXPECTED_TRANSPORT_CASES` guard turns
//! a dropped combination into a failure.
//!
//! # capabilities.tsv
//!
//! This suite deliberately has **no** `capabilities.tsv` row. It is a streaming
//! /transport suite: the streamed routes exercise the request-body transport
//! seam, not the disk-backed body cache, so `body-buffering` is not required.
//! Adding a row "to be safe" would silently skip the suite wherever that
//! capability is unavailable.
//!
//! # Local-stack only
//!
//! The two raw backends (ports 30030 / 30031) are wired into the local launch
//! scripts only, not into `edgion-tests/k8s/**`. See
//! `tasks/todo/transport-coverage-observability-gaps.md`.

use crate::framework::{env_host_or, TestCase, TestContext, TestFuture, TestResult, TestSuite};
use crate::transport::{
    h2_await_reset, h2_reset, h2_send_trailers, probe_err, render_h1_head, RawH1Conn, RawH2Conn, TransportEvent,
    TransportProbe, TransportSnapshot, KIND_CONN_ACCEPTED, KIND_PEER_HALF_CLOSE, KIND_PEER_RESET, KIND_REQ_DATA,
    KIND_REQ_HEADERS, KIND_REQ_TRAILERS, KIND_RESP_DONE, KIND_RESP_HEAD, LISTENER_H1, LISTENER_H2, RECORDER_PORT,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "downstream_timeout.rs"]
mod downstream_timeout;

#[path = "retry_policy.rs"]
mod retry_policy;

#[path = "h2_lifecycle.rs"]
mod h2_lifecycle;

#[path = "h2_lifecycle_connector.rs"]
mod h2_lifecycle_connector;

// ===========================================================================
// The combination matrix
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Proto {
    H1,
    H2,
}

impl Proto {
    fn tag(self) -> &'static str {
        match self {
            Proto::H1 => "h1",
            Proto::H2 => "h2",
        }
    }

    fn listener(self) -> &'static str {
        match self {
            Proto::H1 => LISTENER_H1,
            Proto::H2 => LISTENER_H2,
        }
    }
}

/// One combination: a client constructor (`down`) times a hostname
/// (`up` + `streamed`). No per-combination Gateway or listener YAML exists —
/// the downstream protocol is a client-side choice on one h2c-enabled plain
/// listener, and the upstream protocol is one `appProtocol` Service field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    pub down: Proto,
    pub up: Proto,
    pub streamed: bool,
}

impl Combo {
    const fn new(down: Proto, up: Proto, streamed: bool) -> Self {
        Self { down, up, streamed }
    }

    /// The four ordinary combinations.
    pub const TRANSPORT: [Combo; 4] = [
        Combo::new(Proto::H1, Proto::H1, false),
        Combo::new(Proto::H1, Proto::H2, false),
        Combo::new(Proto::H2, Proto::H1, false),
        Combo::new(Proto::H2, Proto::H2, false),
    ];

    /// The same four with the ExtProc `STREAMED` plugin on the route, which is
    /// what makes Edgion select `RequestRelayPlan::streamed()`.
    pub const STREAMED: [Combo; 4] = [
        Combo::new(Proto::H1, Proto::H1, true),
        Combo::new(Proto::H1, Proto::H2, true),
        Combo::new(Proto::H2, Proto::H1, true),
        Combo::new(Proto::H2, Proto::H2, true),
    ];

    /// The eight, for shapes that assert the whole framing table.
    pub const ALL: [Combo; 8] = [
        Combo::TRANSPORT[0],
        Combo::TRANSPORT[1],
        Combo::TRANSPORT[2],
        Combo::TRANSPORT[3],
        Combo::STREAMED[0],
        Combo::STREAMED[1],
        Combo::STREAMED[2],
        Combo::STREAMED[3],
    ];

    /// Combinations whose upstream is HTTP/2.
    pub const UP_H2: [Combo; 2] = [Combo::TRANSPORT[1], Combo::TRANSPORT[3]];
    /// Combinations whose upstream is HTTP/1.
    pub const UP_H1: [Combo; 2] = [Combo::TRANSPORT[0], Combo::TRANSPORT[2]];
    /// Combinations whose downstream client speaks HTTP/1.
    pub const DOWN_H1: [Combo; 2] = [Combo::TRANSPORT[0], Combo::TRANSPORT[1]];
    /// Combinations whose downstream client speaks HTTP/2.
    pub const DOWN_H2: [Combo; 2] = [Combo::TRANSPORT[2], Combo::TRANSPORT[3]];

    pub fn host(&self) -> &'static str {
        match (self.up, self.streamed) {
            (Proto::H1, false) => "transport-h1.example.com",
            (Proto::H2, false) => "transport-h2.example.com",
            (Proto::H1, true) => "transport-streamed-h1.example.com",
            (Proto::H2, true) => "transport-streamed-h2.example.com",
        }
    }

    pub fn suffix(&self) -> String {
        format!(
            "{}_to_{}{}",
            self.down.tag(),
            self.up.tag(),
            if self.streamed { "_streamed" } else { "" }
        )
    }

    fn up_listener(&self) -> &'static str {
        self.up.listener()
    }
}

// ===========================================================================
// Shapes
// ===========================================================================

type ShapeFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
type ShapeFn = fn(TestContext, Combo, Arc<TransportProbe>) -> ShapeFuture;

/// One transport shape and the combinations it is meaningful for.
struct ShapeDef {
    name: &'static str,
    description: &'static str,
    combos: &'static [Combo],
    /// Combinations this shape cannot express, each with the reason. Emitted as
    /// explicit `..._skipped` cases so a narrowed matrix announces itself.
    skips: &'static [(Combo, &'static str)],
    /// Event kinds this shape's assertions rest on. Verified to be present in
    /// the recorder after the body runs (see `run_transport_case`).
    asserted_kinds: fn(Combo) -> &'static [&'static str],
    /// Event kinds this shape asserts are **absent** for its own trace.
    ///
    /// A negative claim needs a declaration too: without this, a shape named
    /// `client_sends_trailers` could satisfy the naming rule by declaring a kind
    /// it never looks at. The wrapper verifies the absence after the body runs,
    /// so the claim is re-checked outside the shape's own code.
    absent_kinds: &'static [&'static str],
    run: ShapeFn,
}

macro_rules! shape_fn {
    ($f:ident) => {
        (|ctx, combo, probe| Box::pin($f(ctx, combo, probe)) as ShapeFuture) as ShapeFn
    };
}

const K_BASE: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_HEAD];
const K_HEAD_ONLY: &[&str] = &[KIND_REQ_HEADERS];
const K_HEAD_DATA: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA];
const K_HALF_CLOSE_H1: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_DONE, KIND_PEER_HALF_CLOSE];
const K_HALF_CLOSE_H2: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_DONE];
const K_RESET: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_DONE];
const K_RESET_AFTER_RESP: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_HEAD, KIND_RESP_DONE];
const K_TRAILERS: &[&str] = &[KIND_REQ_HEADERS, KIND_REQ_DATA, KIND_RESP_HEAD, KIND_REQ_TRAILERS];

fn kinds_base(_c: Combo) -> &'static [&'static str] {
    K_BASE
}
fn kinds_head_only(_c: Combo) -> &'static [&'static str] {
    K_HEAD_ONLY
}
fn kinds_head_data(_c: Combo) -> &'static [&'static str] {
    K_HEAD_DATA
}
fn kinds_half_close(c: Combo) -> &'static [&'static str] {
    match c.up {
        // The H1 backend FINs its write half and then parks reading, so the
        // gateway's own FIN is recorded as `PeerHalfClose`. HTTP/2 has no
        // per-stream FIN: the backend drops the `RecvStream` instead, which is
        // recorded as the `RespDone` half-close note.
        Proto::H1 => K_HALF_CLOSE_H1,
        Proto::H2 => K_HALF_CLOSE_H2,
    }
}
fn kinds_reset(_c: Combo) -> &'static [&'static str] {
    K_RESET
}
fn kinds_reset_after_response(_c: Combo) -> &'static [&'static str] {
    K_RESET_AFTER_RESP
}
fn kinds_trailers(_c: Combo) -> &'static [&'static str] {
    K_TRAILERS
}

const SKIP_H1_NO_STREAM_RESET: &str =
    "HTTP/1 has no RST_STREAM. The H1 backend's `reset` is a TCP RST, which tears the whole \
     connection down and therefore cannot express 'a complete response followed by a benign \
     stream reset' — the exact shape contract §6's classification fix is about. The H1 \
     equivalent (an unfinished body must not be pooled) is covered by \
     upstream_unfinished_body_poisons_h1_pool.";

const SKIP_H1_NO_RESET_CODE: &str =
    "HTTP/1 has no per-stream error code, so 'reset with a chosen code at a chosen byte offset' \
     is not expressible against an H1 upstream. The H1 backend can only put a TCP RST on the \
     wire, which carries no code and is covered by upstream_half_close_mid_request.";

const SKIP_H2_NO_MALFORMED_FRAMING: &str =
    "The malformed-framing family is HTTP/1-only: HTTP/2 has no chunked framing, and the `h2` \
     crate refuses to emit a message whose framing contradicts itself. The H2 backend degrades \
     these behaviors to `echo` and says so in the RespHead detail, so running them here would \
     assert nothing.";

const SKIP_H2_NO_PIPELINING: &str =
    "Request pipelining is an HTTP/1 wire property. HTTP/2 multiplexes instead, and the \
     multiplexing claim (a second stream on the same connection) is already made by \
     upstream_connection_reuse with an H2 downstream.";

const SKIP_H2_NO_EXPECT_CONTINUE: &str =
    "`Expect: 100-continue` is an HTTP/1 flow-control mechanism. HTTP/2 has no interim-response \
     handshake before DATA (RFC 9113 §8.2.1 leaves `Expect` without meaning), so there is no \
     interim for a downstream H2 client to observe.";

static SHAPES: &[ShapeDef] = &[
    ShapeDef {
        name: "authority_target_forwarding",
        description: "The upstream observes the expected target, query and authority across four protocol combinations",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_head_only,
        run: shape_fn!(shape_authority_target_forwarding),
    },
    ShapeDef {
        name: "h1_authority_rejection_lifecycle",
        description:
            "Two valid requests reuse one HTTP/1 connection; duplicate Host and CONNECT are rejected before upstream",
        combos: &Combo::DOWN_H1,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_head_only,
        run: shape_fn!(shape_h1_authority_rejection_lifecycle),
    },
    ShapeDef {
        name: "recorder_self_test",
        description: "The recorder reports the framing this combination is supposed to produce (runs first)",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_recorder_self_test),
    },
    ShapeDef {
        name: "upstream_early_response",
        description: "Upstream responds before the request body is complete",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_upstream_early_response),
    },
    ShapeDef {
        name: "upstream_half_close_mid_request",
        description: "Upstream half-closes while the request body is still arriving",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_half_close,
        run: shape_fn!(shape_upstream_half_close),
    },
    ShapeDef {
        name: "upstream_reset_no_error_after_complete_response",
        description: "RST_STREAM(NO_ERROR) after a complete response must not destroy the pooled connection",
        combos: &Combo::UP_H2,
        skips: &[
            (Combo::TRANSPORT[0], SKIP_H1_NO_STREAM_RESET),
            (Combo::TRANSPORT[2], SKIP_H1_NO_STREAM_RESET),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_reset_after_response,
        run: shape_fn!(shape_upstream_reset_after_response),
    },
    ShapeDef {
        name: "upstream_reset_at_offset",
        description: "Upstream RST_STREAMs with a chosen code at a chosen request-body byte offset",
        combos: &Combo::UP_H2,
        skips: &[
            (Combo::TRANSPORT[0], SKIP_H1_NO_RESET_CODE),
            (Combo::TRANSPORT[2], SKIP_H1_NO_RESET_CODE),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_reset,
        run: shape_fn!(shape_upstream_reset_at_offset),
    },
    ShapeDef {
        name: "upstream_never_responds",
        description: "Upstream accepts the whole body and never responds; the route timeout must fire",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[KIND_RESP_HEAD],
        asserted_kinds: kinds_head_data,
        run: shape_fn!(shape_upstream_never_responds),
    },
    ShapeDef {
        name: "upstream_request_framing",
        description: "The framing the upstream observes matches the contract §3 row for this combination",
        combos: &Combo::ALL,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_upstream_request_framing),
    },
    ShapeDef {
        name: "upstream_bodyless_streamed_is_coerced_to_ordinary",
        description: "A bodyless streamed request is not re-framed as chunked (the smuggling primitive)",
        combos: &Combo::STREAMED,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_bodyless_streamed_coercion),
    },
    ShapeDef {
        name: "upstream_malformed_response_framing",
        description: "Deliberately malformed upstream response framing produces a deterministic downstream outcome",
        combos: &Combo::UP_H1,
        skips: &[
            (Combo::TRANSPORT[1], SKIP_H2_NO_MALFORMED_FRAMING),
            (Combo::TRANSPORT[3], SKIP_H2_NO_MALFORMED_FRAMING),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_upstream_malformed_framing),
    },
    ShapeDef {
        name: "upstream_connection_reuse",
        description: "Two completed requests share one upstream connection and open exactly one",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_head_only,
        run: shape_fn!(shape_upstream_connection_reuse),
    },
    ShapeDef {
        name: "upstream_unfinished_body_poisons_h1_pool",
        description: "An H1 upstream connection whose request body never completed is never reused",
        combos: &Combo::UP_H1,
        skips: &[
            (
                Combo::TRANSPORT[1],
                "Pool poisoning on an unfinished request body is an HTTP/1 property: an H2 upstream \
                 resets the stream and keeps the connection, which is asserted by \
                 upstream_reset_no_error_after_complete_response instead.",
            ),
            (
                Combo::TRANSPORT[3],
                "Pool poisoning on an unfinished request body is an HTTP/1 property: an H2 upstream \
                 resets the stream and keeps the connection, which is asserted by \
                 upstream_reset_no_error_after_complete_response instead.",
            ),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_head_data,
        run: shape_fn!(shape_unfinished_body_poisons_pool),
    },
    ShapeDef {
        name: "client_disconnect_mid_upload",
        description: "The downstream client aborts mid-upload; the upstream sees a partial body and no retry",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_head_data,
        run: shape_fn!(shape_client_disconnect_mid_upload),
    },
    ShapeDef {
        name: "client_sends_trailers",
        description: "Request trailers are detected but never forwarded to the upstream",
        combos: &Combo::TRANSPORT,
        skips: &[],
        absent_kinds: &[KIND_REQ_TRAILERS],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_client_sends_trailers),
    },
    ShapeDef {
        name: "client_pipelines_second_request",
        description: "A pipelined second HTTP/1 request is never forwarded; request one is answered, the \
                      connection closes, and the client can resend",
        combos: &Combo::DOWN_H1,
        skips: &[
            (Combo::TRANSPORT[2], SKIP_H2_NO_PIPELINING),
            (Combo::TRANSPORT[3], SKIP_H2_NO_PIPELINING),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_client_pipelines),
    },
    ShapeDef {
        name: "client_expect_100_continue",
        description: "No 100 Continue on a route with no body-buffering plugin; the head goes upstream at \
                      once and the body is accepted when the client sends it",
        combos: &Combo::DOWN_H1,
        skips: &[
            (Combo::TRANSPORT[2], SKIP_H2_NO_EXPECT_CONTINUE),
            (Combo::TRANSPORT[3], SKIP_H2_NO_EXPECT_CONTINUE),
        ],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_expect_100_continue),
    },
    ShapeDef {
        name: "streamed_upstream_early_response",
        description: "ExtProc STREAMED intersected with an upstream that responds before the body is complete",
        combos: &Combo::STREAMED,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_base,
        run: shape_fn!(shape_upstream_early_response),
    },
    ShapeDef {
        name: "streamed_client_disconnect_mid_upload",
        description: "ExtProc STREAMED intersected with a downstream client that aborts mid-upload",
        combos: &Combo::STREAMED,
        skips: &[],
        absent_kinds: &[],
        asserted_kinds: kinds_head_data,
        run: shape_fn!(shape_client_disconnect_mid_upload),
    },
];

/// Guard against a silently shrinking matrix.
///
/// The runner rejects a completely empty selection, while this exact count also
/// detects a non-empty matrix that silently loses only some combinations.
const EXPECTED_TRANSPORT_CASES: usize = 165;

// ===========================================================================
// The case wrapper
// ===========================================================================

fn expected_case_count() -> usize {
    SHAPES.iter().map(|s| s.combos.len() + s.skips.len()).sum::<usize>()
        + downstream_timeout::CASE_COUNT
        + retry_policy::CASE_COUNT
        + h2_lifecycle::CASE_COUNT
        + h2_lifecycle_connector::CASE_COUNT
}

pub struct TransportTestSuite;

#[async_trait]
impl TestSuite for TransportTestSuite {
    fn name(&self) -> &str {
        "Transport"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Transport")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        // Fresh-connection retry controls run before the existing shapes warm
        // either upstream pool. Their faults close every exercised connection.
        let mut cases = retry_policy::test_cases();
        cases.extend(h2_lifecycle::test_cases());
        cases.extend(h2_lifecycle_connector::test_cases());
        for shape in SHAPES {
            for combo in shape.combos {
                let combo = *combo;
                cases.push(TestCase::new(
                    format!("{}__{}", shape.name, combo.suffix()),
                    shape.description,
                    move |ctx: TestContext| Box::pin(run_transport_case(ctx, shape, combo)) as TestFuture,
                ));
            }
            for (combo, reason) in shape.skips {
                let combo = *combo;
                let reason = *reason;
                cases.push(TestCase::new(
                    format!("{}__{}__skipped", shape.name, combo.suffix()),
                    shape.description,
                    move |_ctx: TestContext| {
                        Box::pin(async move {
                            TestResult::passed_with_message(
                                Duration::from_millis(0),
                                format!("SKIPPED — this combination is NOT covered. Reason: {reason}"),
                            )
                        }) as TestFuture
                    },
                ));
            }
        }
        cases.extend(downstream_timeout::test_cases());
        assert_eq!(
            cases.len(),
            EXPECTED_TRANSPORT_CASES,
            "the transport matrix changed size: expected {EXPECTED_TRANSPORT_CASES} cases, produced {}. \
             Update EXPECTED_TRANSPORT_CASES deliberately — a silently shrinking matrix still exits 0.",
            cases.len()
        );
        cases
    }
}

/// Run one shape against one combination, with both anti-vacuity guards.
async fn run_transport_case(ctx: TestContext, shape: &'static ShapeDef, combo: Combo) -> TestResult {
    let start = Instant::now();
    let label = format!("{}-{}", shape.name, combo.suffix());
    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = match TransportProbe::start(&recorder_host, RECORDER_PORT, &label).await {
        Ok(probe) => Arc::new(probe),
        Err(e) => return TestResult::failed(start.elapsed(), probe_err("transport probe could not start", e)),
    };

    let message = match (shape.run)(ctx.clone(), combo, probe.clone()).await {
        Ok(message) => message,
        Err(e) => return TestResult::failed(start.elapsed(), e),
    };

    // Guard 1 — the upstream must actually have been exercised. Without this a
    // case whose request never left the gateway would pass on assertions that
    // are all vacuously true.
    if let Err(e) = probe
        .wait_for(
            "a ReqHeaders event proving the upstream received this request",
            Duration::from_secs(3),
            |e| e.is(KIND_REQ_HEADERS),
        )
        .await
    {
        return TestResult::failed(
            start.elapsed(),
            format!(
                "this case finished without the upstream ever receiving a request — its assertions are vacuous\n{e}"
            ),
        );
    }

    // Guard 2 — every declared kind must actually be present.
    let snapshot = match probe.snapshot().await {
        Ok(s) => s,
        Err(e) => return TestResult::failed(start.elapsed(), probe_err("final snapshot failed", e)),
    };
    for kind in (shape.asserted_kinds)(combo) {
        if snapshot.count_matching(|e| e.is(kind)) == 0 {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "shape `{}` declares it asserts on `{kind}` events, but the recorder never saw one for {} \
                     — the case is named for something it could not observe\n{snapshot}",
                    shape.name,
                    combo.suffix()
                ),
            );
        }
    }

    for kind in shape.absent_kinds {
        let seen = snapshot.count_matching(|e| e.is(kind) && e.tagged(probe.trace_id()));
        if seen != 0 {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "shape `{}` claims no `{kind}` event reaches the upstream for {}, but the recorder has \
                     {seen}\n{snapshot}",
                    shape.name,
                    combo.suffix()
                ),
            );
        }
    }

    TestResult::passed_with_message(start.elapsed(), message)
}

// ===========================================================================
// Client driving helpers
// ===========================================================================

/// How the downstream client frames its request body.
#[derive(Clone)]
enum BodyPlan {
    /// No body at all: H1 declares neither Content-Length nor Transfer-Encoding;
    /// H2 sets END_STREAM on HEADERS.
    Bodyless,
    /// A declared, complete body.
    ContentLength(Vec<u8>),
    /// A streamed body: H1 chunked, H2 one DATA frame per chunk.
    Chunked { chunks: Vec<Vec<u8>>, pace: Duration },
    /// A streamed body the client **abandons** the moment the peer answers:
    /// paced chunks, and no terminator is ever written.
    ///
    /// This is the client half of RFC 9113 §8.1. A shape whose premise is "the
    /// origin answers while the client is still uploading" needs a client that
    /// is genuinely still uploading when the answer arrives — and one that then
    /// stops, because a client that keeps writing into a connection the gateway
    /// has already finished with turns its own writes into a reset that
    /// destroys the response it was about to read.
    ChunkedUntilResponse { chunks: Vec<Vec<u8>>, pace: Duration },
    /// A streamed body finished with a **real** trailer section.
    ChunkedWithTrailers {
        chunks: Vec<Vec<u8>>,
        trailers: Vec<(String, String)>,
    },
}

#[derive(Default)]
struct ClientOutcome {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Non-fatal transport errors seen while driving. A peer that closes early
    /// is a legitimate observation for several shapes, so these are collected
    /// rather than raised.
    errors: Vec<String>,
}

impl ClientOutcome {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn describe(&self) -> String {
        format!(
            "downstream: status={:?} errors={:?} body={:?}",
            self.status,
            self.errors,
            String::from_utf8_lossy(&self.body)
                .chars()
                .take(120)
                .collect::<String>()
        )
    }
}

fn ctrl(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn chunks_of(count: usize, size: usize, seed: u8) -> Vec<Vec<u8>> {
    (0..count).map(|i| vec![seed.wrapping_add(i as u8); size]).collect()
}

/// Drive one request over the combination's downstream protocol.
async fn drive(
    ctx: &TestContext,
    combo: Combo,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    plan: BodyPlan,
    read_timeout: Duration,
) -> Result<ClientOutcome, String> {
    match combo.down {
        Proto::H1 => drive_h1(ctx, combo, method, path, headers, plan, read_timeout).await,
        Proto::H2 => drive_h2(ctx, combo, method, path, headers, plan, read_timeout).await,
    }
}

async fn drive_h1(
    ctx: &TestContext,
    combo: Combo,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    plan: BodyPlan,
    read_timeout: Duration,
) -> Result<ClientOutcome, String> {
    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    let mut outcome = ClientOutcome::default();

    let content_length;
    let mut head: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match &plan {
        BodyPlan::Bodyless => {}
        BodyPlan::ContentLength(body) => {
            content_length = body.len().to_string();
            head.push(("Content-Length", &content_length));
        }
        BodyPlan::Chunked { .. } | BodyPlan::ChunkedUntilResponse { .. } | BodyPlan::ChunkedWithTrailers { .. } => {
            head.push(("Transfer-Encoding", "chunked"));
        }
    }
    conn.write_head(method, path, combo.host(), &head).await?;

    match &plan {
        BodyPlan::Bodyless => {}
        BodyPlan::ContentLength(body) => {
            if let Err(e) = conn.write_raw(body).await {
                outcome.errors.push(format!("body write: {e}"));
            }
        }
        BodyPlan::Chunked { chunks, pace } => {
            for chunk in chunks {
                if let Err(e) = conn.write_chunk(chunk).await {
                    outcome.errors.push(format!("chunk write: {e}"));
                    break;
                }
                if !pace.is_zero() {
                    tokio::time::sleep(*pace).await;
                }
            }
            if let Err(e) = conn.finish_chunked().await {
                outcome.errors.push(format!("chunk terminator write: {e}"));
            }
        }
        BodyPlan::ChunkedUntilResponse { chunks, pace } => {
            // The pacing gap doubles as the read window, so watching for the
            // response costs no extra latency. No terminator is ever written:
            // this client abandons its upload, which is the premise.
            for chunk in chunks {
                if let Err(e) = conn.write_chunk(chunk).await {
                    outcome.errors.push(format!("chunk write: {e}"));
                    break;
                }
                match conn.read_available(*pace).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => {
                        outcome.errors.push(format!("read while uploading: {e}"));
                        break;
                    }
                }
            }
        }
        BodyPlan::ChunkedWithTrailers { chunks, trailers } => {
            for chunk in chunks {
                if let Err(e) = conn.write_chunk(chunk).await {
                    outcome.errors.push(format!("chunk write: {e}"));
                    break;
                }
            }
            let refs: Vec<(&str, &str)> = trailers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            if let Err(e) = conn.finish_chunked_with_trailers(&refs).await {
                outcome.errors.push(format!("trailer write: {e}"));
            }
        }
    }

    match conn.read_response_head(read_timeout).await {
        Ok(head) => {
            outcome.status = Some(head.status);
            outcome.headers = head.headers.clone();
            match conn.read_body(&head, read_timeout).await {
                Ok(body) => outcome.body = body,
                Err(e) => outcome.errors.push(format!("response body read: {e}")),
            }
        }
        Err(e) => outcome.errors.push(format!("response head read: {e}")),
    }
    Ok(outcome)
}

async fn drive_h2(
    ctx: &TestContext,
    combo: Combo,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    plan: BodyPlan,
    read_timeout: Duration,
) -> Result<ClientOutcome, String> {
    let mut conn = RawH2Conn::connect(&ctx.target_host, ctx.http_port).await?;
    let mut outcome = ClientOutcome::default();

    let uri = format!("http://{}:{}{}", combo.host(), ctx.http_port, path);
    let content_length;
    let mut head: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    if let BodyPlan::ContentLength(body) = &plan {
        content_length = body.len().to_string();
        head.push(("content-length", &content_length));
    }
    let end_stream = matches!(plan, BodyPlan::Bodyless);
    let (response, mut send) = conn.send_request(method, &uri, &head, end_stream)?;
    // Pinned so the upload loop below can poll it *while* it writes: a plan that
    // stops uploading when the answer arrives has to be able to see the answer.
    let mut response = Box::pin(response);
    let mut early = None;

    match &plan {
        BodyPlan::Bodyless => {}
        BodyPlan::ContentLength(body) => {
            if let Err(e) = send.send_data(Bytes::from(body.clone()), true) {
                outcome.errors.push(format!("DATA write: {e}"));
            }
        }
        BodyPlan::Chunked { chunks, pace } => {
            for chunk in chunks {
                send.reserve_capacity(chunk.len());
                if let Err(e) = send.send_data(Bytes::from(chunk.clone()), false) {
                    outcome.errors.push(format!("DATA write: {e}"));
                    break;
                }
                if !pace.is_zero() {
                    tokio::time::sleep(*pace).await;
                }
            }
            if let Err(e) = send.send_data(Bytes::new(), true) {
                outcome.errors.push(format!("END_STREAM write: {e}"));
            }
        }
        BodyPlan::ChunkedUntilResponse { chunks, pace } => {
            // The pacing gap doubles as the window in which the response may
            // arrive. END_STREAM is never sent: this client abandons its upload.
            for chunk in chunks {
                send.reserve_capacity(chunk.len());
                if let Err(e) = send.send_data(Bytes::from(chunk.clone()), false) {
                    outcome.errors.push(format!("DATA write: {e}"));
                    break;
                }
                if let Ok(result) = tokio::time::timeout(*pace, &mut response).await {
                    early = Some(result);
                    break;
                }
            }
        }
        BodyPlan::ChunkedWithTrailers { chunks, trailers } => {
            for chunk in chunks {
                send.reserve_capacity(chunk.len());
                if let Err(e) = send.send_data(Bytes::from(chunk.clone()), false) {
                    outcome.errors.push(format!("DATA write: {e}"));
                    break;
                }
            }
            let mut map = http::HeaderMap::new();
            for (name, value) in trailers {
                if let (Ok(n), Ok(v)) = (
                    name.parse::<http::header::HeaderName>(),
                    value.parse::<http::header::HeaderValue>(),
                ) {
                    map.insert(n, v);
                }
            }
            if let Err(e) = h2_send_trailers(&mut send, map) {
                outcome.errors.push(format!("trailer write: {e}"));
            }
        }
    }

    let received = match early {
        Some(result) => Ok(result),
        None => tokio::time::timeout(read_timeout, response).await,
    };
    match received {
        Ok(Ok(resp)) => {
            outcome.status = Some(resp.status().as_u16());
            outcome.headers = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect();
            let mut body = resp.into_body();
            while let Some(chunk) = body.data().await {
                match chunk {
                    Ok(data) => {
                        let _ = body.flow_control().release_capacity(data.len());
                        outcome.body.extend_from_slice(&data);
                    }
                    Err(e) => {
                        outcome.errors.push(format!("response body read: {e}"));
                        break;
                    }
                }
            }
        }
        Ok(Err(e)) => outcome.errors.push(format!("response: {e}")),
        Err(_) => outcome.errors.push(format!("no response within {read_timeout:?}")),
    }

    conn.shutdown();
    Ok(outcome)
}

// ===========================================================================
// Shared assertion helpers
// ===========================================================================

/// Require that exactly one upstream request head carried this trace — no retry,
/// no duplicate attempt. An exact count over the whole log, never a bound.
fn require_single_attempt(snapshot: &TransportSnapshot, trace: &str) -> Result<(), String> {
    let attempts = snapshot.count_matching(|e| e.is(KIND_REQ_HEADERS) && e.tagged(trace));
    if attempts != 1 {
        return Err(format!(
            "expected exactly 1 upstream request head for trace {trace}, saw {attempts}\n{snapshot}"
        ));
    }
    Ok(())
}

fn require_listener(head: &TransportEvent, combo: Combo) -> Result<(), String> {
    if head.listener != combo.up_listener() {
        return Err(format!(
            "expected the request to reach the {} upstream backend, but it reached `{}` \
             (appProtocol selection or route matching is wrong; every later assertion in this \
             shape would be about the wrong backend): {head}",
            combo.up_listener(),
            head.listener
        ));
    }
    Ok(())
}

async fn head_for(probe: &TransportProbe, trace: &str, combo: Combo) -> Result<TransportEvent, String> {
    let head = probe
        .wait_for_trace(trace, "the upstream request head", Duration::from_secs(5), move |e| {
            e.is(KIND_REQ_HEADERS) && e.tagged(trace)
        })
        .await?;
    require_listener(&head, combo)?;
    Ok(head)
}

// ===========================================================================
// Shapes — upstream side
// ===========================================================================

async fn shape_authority_target_forwarding(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let origin_target = "/%61uthority-check?scope=read";
    let normalized_target = "/authority-check?scope=read";
    let client_target = if combo.down == Proto::H1 {
        format!("http://{}{origin_target}", combo.host())
    } else {
        origin_target.to_string()
    };
    let outcome = drive(
        &ctx,
        combo,
        "GET",
        &client_target,
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::Bodyless,
        Duration::from_secs(5),
    )
    .await?;
    if outcome.status != Some(200) {
        return Err(format!("valid authority request failed: {}", outcome.describe()));
    }

    let head = head_for(&probe, &trace, combo).await?;
    let expected_target = if combo.down == Proto::H1 && combo.up == Proto::H1 {
        format!("http://{}{normalized_target}", combo.host())
    } else {
        normalized_target.to_string()
    };
    if head.target.as_deref() != Some(expected_target.as_str()) {
        return Err(format!(
            "upstream target differs: expected {expected_target}, observed {:?}",
            head.target
        ));
    }
    let expected_authority = if combo.down == Proto::H2 {
        format!("{}:{}", combo.host(), ctx.http_port)
    } else {
        combo.host().to_string()
    };
    if head.authority.as_deref() != Some(expected_authority.as_str()) {
        return Err(format!(
            "upstream authority differs: expected {expected_authority}, observed {:?}",
            head.authority
        ));
    }
    Ok(format!(
        "upstream {} target={expected_target} authority={expected_authority}",
        combo.up.tag()
    ))
}

async fn shape_h1_authority_rejection_lifecycle(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let valid_trace = probe.trace_id().to_string();
    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    for _ in 0..2 {
        conn.write_head(
            "GET",
            "/authority-reuse",
            combo.host(),
            &[
                ("x-up-behavior", "echo"),
                (
                    "traceparent",
                    &crate::access_log_client::traceparent_for_marker(&valid_trace),
                ),
            ],
        )
        .await?;
        let response = conn.read_response_head(Duration::from_secs(5)).await?;
        if response.status != 200 {
            return Err(format!("valid reused request returned {}", response.status));
        }
        conn.read_body(&response, Duration::from_secs(5)).await?;
    }
    let snapshot = probe.snapshot_of(&valid_trace).await?;
    if snapshot.count_matching(|event| event.is(KIND_REQ_HEADERS) && event.tagged(&valid_trace)) != 2 {
        return Err("two valid requests did not reach the upstream on one downstream connection".to_string());
    }

    let bad_trace = format!("{valid_trace}-duplicate-host");
    let bad_request = render_h1_head(
        "GET",
        "/authority-reuse",
        combo.host(),
        &[
            ("Host", "other.example.com"),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&bad_trace),
            ),
        ],
    );
    conn.write_raw(bad_request.as_bytes()).await?;
    let rejected = conn.read_response_head(Duration::from_secs(5)).await?;
    if rejected.status != 400 || !conn.wait_for_close(Duration::from_secs(2)).await? {
        return Err(format!(
            "duplicate Host did not return 400 and close the HTTP/1 connection: {rejected}"
        ));
    }
    probe
        .expect_none_for(
            &bad_trace,
            "duplicate Host upstream request",
            Duration::from_millis(300),
            |event| event.is(KIND_REQ_HEADERS) && event.tagged(&bad_trace),
        )
        .await?;
    if probe
        .snapshot_of(&bad_trace)
        .await?
        .count_matching(|event| event.is(KIND_REQ_HEADERS) && event.tagged(&bad_trace))
        != 0
    {
        return Err("duplicate Host reached the upstream".to_string());
    }

    let connect_trace = format!("{valid_trace}-connect");
    let mut connect = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    let connect_request = format!(
        "CONNECT {}:443 HTTP/1.1\r\nHost: {}:443\r\ntraceparent: {}\r\n\r\n",
        combo.host(),
        combo.host(),
        crate::access_log_client::traceparent_for_marker(&connect_trace)
    );
    connect.write_raw(connect_request.as_bytes()).await?;
    let refused = connect.read_response_head(Duration::from_secs(5)).await?;
    if refused.status != 405 {
        return Err(format!(
            "CONNECT admission changed: expected 405, got {}",
            refused.status
        ));
    }
    probe
        .expect_none_for(
            &connect_trace,
            "CONNECT upstream request",
            Duration::from_millis(300),
            |event| event.is(KIND_REQ_HEADERS) && event.tagged(&connect_trace),
        )
        .await?;
    if probe
        .snapshot_of(&connect_trace)
        .await?
        .count_matching(|event| event.is(KIND_REQ_HEADERS) && event.tagged(&connect_trace))
        != 0
    {
        return Err("refused CONNECT reached the upstream".to_string());
    }
    Ok(
        "two valid requests reused one H1 connection; duplicate Host closed it and CONNECT remained disabled"
            .to_string(),
    )
}

async fn shape_recorder_self_test(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let body = b"self-test-body-0123456789".to_vec();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/self-test",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::ContentLength(body.clone()),
        Duration::from_secs(10),
    )
    .await?;

    if out.status != Some(200) {
        return Err(format!("expected 200 from the echo script, {}", out.describe()));
    }
    let reported_version = out.header("x-up-http-version").unwrap_or("<absent>");
    let expected_version = match combo.up {
        Proto::H1 => "1.1",
        Proto::H2 => "2",
    };
    if reported_version != expected_version {
        return Err(format!(
            "the backend reported HTTP version {reported_version}, expected {expected_version} — the \
             upstream protocol for this combination is not what the fixtures claim. {}",
            out.describe()
        ));
    }
    if out.header("x-up-req-body-len") != Some(body.len().to_string().as_str()) {
        return Err(format!(
            "the backend saw {:?} body bytes, expected {}. {}",
            out.header("x-up-req-body-len"),
            body.len(),
            out.describe()
        ));
    }

    let head = head_for(&probe, &trace, combo).await?;
    if head.content_length != Some(body.len() as u64) {
        return Err(format!(
            "expected the upstream request to declare Content-Length {}, recorder saw {:?}: {head}",
            body.len(),
            head.content_length
        ));
    }
    if head.headers_eos != Some(false) {
        return Err(format!(
            "a body-bearing request must not carry headers-EOS, recorder saw {:?}: {head}",
            head.headers_eos
        ));
    }
    let eos = probe
        .wait_for(
            "the request body to complete at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true),
        )
        .await?;

    let snapshot = probe.snapshot().await?;
    let total = snapshot.req_body_total(&trace);
    if total != body.len() {
        return Err(format!(
            "the recorder counted {total} upstream body bytes, expected {}\n{snapshot}",
            body.len()
        ));
    }
    require_single_attempt(&snapshot, &trace)?;
    probe
        .expect_none(
            "a second upstream request head for this trace (a retry)",
            Duration::from_millis(400),
            |e| e.is(KIND_REQ_HEADERS) && e.tagged(&trace),
        )
        .await?;

    Ok(format!(
        "recorder agrees with the wire: {} upstream, CL={:?}, headers_eos=false, {} body bytes, eos at seq {}",
        combo.up_listener(),
        head.content_length,
        total,
        eos.seq
    ))
}

async fn shape_upstream_early_response(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/early",
        &ctrl(&[
            ("x-up-behavior", "early-response"),
            ("x-up-at-bytes", "0"),
            ("x-up-drain", "true"),
            ("x-up-status", "200"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::Chunked {
            chunks: chunks_of(5, 32, b'e'),
            pace: Duration::from_millis(120),
        },
        Duration::from_secs(10),
    )
    .await?;

    let head = head_for(&probe, &trace, combo).await?;
    let resp_head = probe
        .wait_for("the upstream response head", Duration::from_secs(5), |e| {
            e.is(KIND_RESP_HEAD) && e.tagged(&trace)
        })
        .await?;

    let snapshot = probe.snapshot().await?;
    // The ordering claim, anchored on the recorder's monotonic sequence rather
    // than on a sleep: the response head must precede request-body completion,
    // or the body must never have completed at all (futile-read termination).
    if let Some(eos) = snapshot.find(|e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true)) {
        if resp_head.seq > eos.seq {
            return Err(format!(
                "the upstream responded only AFTER the request body completed (RespHead seq {} > eos seq {}); \
                 this case cannot be asserting an early response\n{snapshot}",
                resp_head.seq, eos.seq
            ));
        }
    }
    if out.status != Some(200) {
        return Err(format!(
            "expected the early 200 to reach the client, {}\n{snapshot}",
            out.describe()
        ));
    }
    require_single_attempt(&snapshot, &trace)?;
    probe
        .expect_none(
            "a retry attempt after the response was committed",
            Duration::from_millis(500),
            |e| e.is(KIND_REQ_HEADERS) && e.tagged(&trace),
        )
        .await?;

    let eos_note = match snapshot.find(|e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true)) {
        Some(e) => format!("body completed at seq {}", e.seq),
        None => "body never completed (futile-read termination)".to_string(),
    };
    Ok(format!(
        "early response at seq {} preceded {eos_note}; upstream saw {} body bytes on {}",
        resp_head.seq,
        snapshot.req_body_total(&trace),
        head.listener
    ))
}

async fn shape_upstream_half_close(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/half-close",
        &ctrl(&[
            ("x-up-behavior", "half-close"),
            ("x-up-at-bytes", "32"),
            ("x-up-drain", "true"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::Chunked {
            chunks: chunks_of(6, 16, b'h'),
            pace: Duration::from_millis(60),
        },
        Duration::from_secs(10),
    )
    .await?;

    let head = head_for(&probe, &trace, combo).await?;
    // Prove the upstream really received the request body before asserting
    // anything about how it ended.
    let _ = probe
        .wait_for(
            "at least one request-body chunk at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.len.unwrap_or(0) > 0,
        )
        .await?;
    let half_close = probe
        .wait_for(
            "the backend's record of performing the half-close",
            Duration::from_secs(5),
            |e| {
                e.is(KIND_RESP_DONE)
                    && e.tagged(&trace)
                    && e.detail.as_deref().map(|d| d.contains("half-close")).unwrap_or(false)
            },
        )
        .await?;

    let snapshot = probe.snapshot().await?;
    let total = snapshot.req_body_total(&trace);
    if total < 32 {
        return Err(format!(
            "the half-close was scripted at 32 request-body bytes but the upstream only recorded {total}; \
             the case did not reach the state it is named for\n{snapshot}"
        ));
    }

    // A deterministic downstream outcome, not a hang. Which outcome depends on
    // the upstream protocol, and both are asserted exactly.
    match combo.up {
        Proto::H2 => {
            // The H2 backend completes the response first, then drops the
            // receive stream, so the client must see the response.
            if out.status != Some(200) {
                return Err(format!(
                    "an H2 upstream half-close completes the response first; expected 200, {}\n{snapshot}",
                    out.describe()
                ));
            }
        }
        Proto::H1 => {
            // The H1 backend FINs its write half with no response at all, so
            // the gateway must surface an error rather than a truncated 200.
            match out.status {
                Some(status) if (500..600).contains(&status) => {}
                Some(status) => {
                    return Err(format!(
                        "an H1 upstream that half-closed without responding must not produce {status}\n{snapshot}"
                    ))
                }
                None if !out.errors.is_empty() => {}
                None => {
                    return Err(format!(
                        "the downstream request neither completed nor failed — a hang, not a deterministic \
                         outcome\n{snapshot}"
                    ))
                }
            }
        }
    }
    require_single_attempt(&snapshot, &trace)?;

    Ok(format!(
        "upstream {} half-closed after {total} body bytes (seq {}), downstream outcome: {}",
        head.listener,
        half_close.seq,
        out.describe()
    ))
}

/// The response body length this shape scripts. Named because the case asserts
/// it downstream: "a complete response" is a claim about the body, not the head.
const RESET_RESP_LEN: usize = 64;

/// Gap 9 in `tasks/todo/transport-coverage-observability-gaps.md`.
///
/// The three things this shape has to get right, none of which is optional:
///
///  * the **upstream** must really put `HEADERS`, `DATA(END_STREAM)` and then
///    `RST_STREAM(NO_ERROR)` on the wire, in that order — see the comment on the
///    control headers below, and `server::transport::h2::wire_proof` for the
///    frame-level proof that it does;
///  * the **downstream client** must still be uploading when that happens, which
///    is RFC 9113 §8.1's premise, and must then stop rather than keep writing
///    into a connection the gateway has finished with (`ChunkedUntilResponse`);
///  * the **gateway** must deliver the complete 200 downstream and keep the
///    pooled upstream connection. That is the product claim, and it is not to be
///    weakened: if it fails, it is a finding, not a test to adjust.
///
/// The race that decides the first bullet is tight and real: the gateway answers
/// a complete response by sending `RST_STREAM(CANCEL)` upstream to stop the
/// upload it no longer needs, measured at ~2 ms. The backend's reset has to be
/// out before that, which is why its flush barrier is a round trip through the
/// connection loop and not a sleep.
async fn shape_upstream_reset_after_response(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace_a = probe.trace_id().to_string();
    let trace_b = probe.sibling_trace("second");

    let first = drive(
        &ctx,
        combo,
        "POST",
        "/reset-after-response",
        &ctrl(&[
            // `echo` cannot express this shape and never could: it drains the
            // whole request body first, which leaves the upstream stream closed,
            // and `h2` answers `send_reset` on a closed stream by emitting
            // nothing (`Send::send_reset`, `is_closed && is_empty`). RFC 9113
            // §8.1's premise is an origin that answers *while the client is
            // still uploading*, so that is what is scripted here:
            // `early-response` at the first body bytes, `x-up-drain: false` so
            // the request stream stays open, and only then the reset.
            // `server::transport::h2::wire_proof` holds both halves of that to
            // the wire.
            ("x-up-behavior", "early-response"),
            ("x-up-at-bytes", "32"),
            ("x-up-drain", "false"),
            ("x-up-after-response", "reset"),
            ("x-up-reset-code", "NO_ERROR"),
            ("x-up-resp-len", &RESET_RESP_LEN.to_string()),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&trace_a),
            ),
        ]),
        BodyPlan::ChunkedUntilResponse {
            chunks: chunks_of(12, 64, b'q'),
            pace: Duration::from_millis(80),
        },
        Duration::from_secs(10),
    )
    .await?;
    if first.status != Some(200) {
        return Err(format!(
            "a complete response followed by RST_STREAM(NO_ERROR) must still be a 200 downstream, {}",
            first.describe()
        ));
    }
    // "Complete" is the word the claim turns on: a head is not enough, the whole
    // body has to arrive downstream too.
    if first.body.len() != RESET_RESP_LEN {
        return Err(format!(
            "a complete response followed by RST_STREAM(NO_ERROR) must be delivered downstream in full: \
             expected {RESET_RESP_LEN} body bytes, got {}, {}",
            first.body.len(),
            first.describe()
        ));
    }

    let head_a = head_for(&probe, &trace_a, combo).await?;
    let reset = probe
        .wait_for_trace(
            &trace_a,
            "the backend's RST_STREAM(NO_ERROR) after the complete response",
            Duration::from_secs(5),
            |e| e.is(KIND_RESP_DONE) && e.tagged(&trace_a) && e.code.as_deref() == Some("NO_ERROR"),
        )
        .await?;
    // Exactly one, over the whole log. The backend records this event only after
    // `send_reset` has run and only when the stream was still open — the state
    // in which `h2` really does queue the frame — so a run where the reset was
    // suppressed leaves no such event and cannot satisfy this count.
    let claims = probe
        .snapshot()
        .await?
        .count_matching(|e| e.is(KIND_RESP_DONE) && e.tagged(&trace_a) && e.code.as_deref() == Some("NO_ERROR"));
    if claims != 1 {
        return Err(format!(
            "expected exactly 1 RST_STREAM(NO_ERROR) record for trace {trace_a}, saw {claims}\n{}",
            probe.snapshot_of(&trace_a).await?
        ));
    }

    let before = probe.snapshot().await?.conn_accepted_total(combo.up_listener());

    // The positive twin: a second request must land on the SAME connection.
    // Contract §6 — the benign classification must not destroy a healthy pooled
    // connection.
    let second = drive(
        &ctx,
        combo,
        "POST",
        "/reset-after-response",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&trace_b),
            ),
        ]),
        BodyPlan::ContentLength(b"follow-up".to_vec()),
        Duration::from_secs(10),
    )
    .await?;
    if second.status != Some(200) {
        return Err(format!(
            "the follow-up request on the pooled connection failed, {}",
            second.describe()
        ));
    }
    let head_b = head_for(&probe, &trace_b, combo).await?;
    if head_b.conn != head_a.conn {
        return Err(format!(
            "expected the pooled {} connection {} to be reused after the benign NO_ERROR reset, \
             but the follow-up landed on connection {}\n{}",
            combo.up_listener(),
            head_a.conn,
            head_b.conn,
            probe.snapshot_of(&trace_b).await?
        ));
    }

    // The negative half: no new upstream connection was opened for it.
    let after = probe
        .snapshot_of(&trace_b)
        .await?
        .conn_accepted_total(combo.up_listener());
    if after != before {
        return Err(format!(
            "a benign NO_ERROR reset must not force a new upstream connection: {} accepts before the \
             follow-up, {} after",
            before, after
        ));
    }
    probe
        .expect_none(
            "a new upstream connection after the benign reset",
            Duration::from_millis(400),
            |e| e.is(KIND_CONN_ACCEPTED),
        )
        .await?;

    Ok(format!(
        "RST_STREAM(NO_ERROR) at seq {} after a complete response; conn {} reused by the follow-up, \
         accept counter unchanged at {}",
        reset.seq, head_a.conn, after
    ))
}

async fn shape_upstream_reset_at_offset(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/reset-at-offset",
        &ctrl(&[
            ("x-up-behavior", "reset"),
            ("x-up-at-bytes", "48"),
            ("x-up-reset-code", "INTERNAL_ERROR"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::Chunked {
            chunks: chunks_of(8, 16, b'r'),
            pace: Duration::from_millis(40),
        },
        Duration::from_secs(10),
    )
    .await?;

    let head = head_for(&probe, &trace, combo).await?;
    let _ = probe
        .wait_for(
            "at least one request-body chunk at the upstream before the reset",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.len.unwrap_or(0) > 0,
        )
        .await?;
    let reset = probe
        .wait_for(
            "the backend's RST_STREAM with the requested code",
            Duration::from_secs(5),
            |e| e.is(KIND_RESP_DONE) && e.tagged(&trace) && e.code.is_some(),
        )
        .await?;
    if reset.code.as_deref() != Some("INTERNAL_ERROR") {
        return Err(format!(
            "expected the upstream to reset with INTERNAL_ERROR, recorder saw {:?}: {reset}",
            reset.code
        ));
    }

    let snapshot = probe.snapshot().await?;
    let total = snapshot.req_body_total(&trace);
    if total < 48 {
        return Err(format!(
            "the reset was scripted at 48 request-body bytes but only {total} arrived; the case did not \
             reach the state it is named for\n{snapshot}"
        ));
    }
    // The error must surface, not be papered over with a truncated success.
    match out.status {
        Some(200) => {
            return Err(format!(
                "an upstream INTERNAL_ERROR reset before any response must not surface as 200\n{snapshot}"
            ))
        }
        Some(status) if (500..600).contains(&status) => {}
        Some(status) => {
            return Err(format!(
                "expected a 5xx after the upstream reset, got {status}. {}\n{snapshot}",
                out.describe()
            ))
        }
        None if !out.errors.is_empty() => {}
        None => {
            return Err(format!(
                "the downstream request neither completed nor failed after the upstream reset\n{snapshot}"
            ))
        }
    }
    require_single_attempt(&snapshot, &trace)?;

    Ok(format!(
        "upstream {} reset with INTERNAL_ERROR at {total} body bytes (seq {}); downstream {}",
        head.listener,
        reset.seq,
        out.describe()
    ))
}

async fn shape_upstream_never_responds(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let started = Instant::now();
    // `/timeout` is the route rule carrying `backendRequest: 2s`, so the
    // outcome is the route's configured status rather than a global default.
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/timeout/hang",
        &ctrl(&[
            ("x-up-behavior", "hang"),
            ("x-up-drain", "true"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::ContentLength(b"this-body-arrives-but-no-response-ever-does".to_vec()),
        Duration::from_secs(15),
    )
    .await?;
    let elapsed = started.elapsed();

    let head = head_for(&probe, &trace, combo).await?;
    // The whole body must have reached the upstream — otherwise "never
    // responds" would be indistinguishable from "never received".
    let eos = probe
        .wait_for(
            "the complete request body at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true),
        )
        .await?;

    if out.status != Some(504) {
        return Err(format!(
            "expected the route's backendRequest timeout to produce 504, {} (after {elapsed:?})",
            out.describe()
        ));
    }
    if elapsed > Duration::from_secs(10) {
        return Err(format!(
            "the 504 arrived after {elapsed:?}; the route timeout is 2s, so this was not the timeout firing"
        ));
    }

    let snapshot = probe.snapshot().await?;
    // No EOS was synthesised downstream-side into a second attempt.
    require_single_attempt(&snapshot, &trace)?;
    probe
        .expect_none(
            "a retry attempt after the upstream timed out",
            Duration::from_millis(400),
            |e| e.is(KIND_REQ_HEADERS) && e.tagged(&trace),
        )
        .await?;
    if snapshot.count_matching(|e| e.is(KIND_RESP_HEAD) && e.tagged(&trace)) != 0 {
        return Err(format!(
            "the `hang` script must never write a response head, but the recorder has one\n{snapshot}"
        ));
    }

    Ok(format!(
        "upstream {} received the whole body (eos seq {}) and never responded; 504 after {elapsed:?}",
        head.listener, eos.seq
    ))
}

async fn shape_upstream_request_framing(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let body = b"framing-matrix-request-body".to_vec();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/framing",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::ContentLength(body.clone()),
        Duration::from_secs(10),
    )
    .await?;
    if out.status != Some(200) {
        return Err(format!("expected 200 from the echo script, {}", out.describe()));
    }

    let head = head_for(&probe, &trace, combo).await?;
    let te = head.transfer_encoding.as_deref().unwrap_or("");
    let chunked = te.to_ascii_lowercase().contains("chunked");

    match (combo.up, combo.streamed) {
        // Contract §3, Ordinary row: the declaration the client made is what
        // the upstream observes.
        (Proto::H1, false) => {
            if head.content_length != Some(body.len() as u64) {
                return Err(format!(
                    "Ordinary + H1 upstream must forward the declared Content-Length {}, recorder saw {:?}: {head}",
                    body.len(),
                    head.content_length
                ));
            }
            if chunked {
                return Err(format!(
                    "Ordinary + H1 upstream must not re-frame a declared body as chunked: {head}"
                ));
            }
        }
        (Proto::H2, false) => {
            if head.headers_eos != Some(false) {
                return Err(format!(
                    "a body-bearing Ordinary request must carry END_STREAM=false on HEADERS: {head}"
                ));
            }
            if chunked {
                return Err(format!("HTTP/2 must never carry Transfer-Encoding: {head}"));
            }
        }
        // Contract §3, Streamed row.
        (Proto::H1, true) => {
            if !chunked {
                return Err(format!(
                    "Streamed + H1 upstream must carry `Transfer-Encoding: chunked`, recorder saw te={:?}: {head}",
                    head.transfer_encoding
                ));
            }
            if head.content_length.is_some() {
                return Err(format!(
                    "Streamed + H1 upstream must NOT carry Content-Length alongside chunked (the smuggling \
                     primitive), recorder saw {:?}: {head}",
                    head.content_length
                ));
            }
        }
        (Proto::H2, true) => {
            if head.headers_eos != Some(false) {
                return Err(format!(
                    "Streamed + H2 upstream must set END_STREAM=false on HEADERS ALWAYS, recorder saw {:?}: {head}",
                    head.headers_eos
                ));
            }
            if head.content_length.is_some() || !te.is_empty() {
                return Err(format!(
                    "Streamed + H2 upstream must carry neither Content-Length nor Transfer-Encoding, \
                     recorder saw cl={:?} te={:?}: {head}",
                    head.content_length, head.transfer_encoding
                ));
            }
        }
    }

    let _ = probe
        .wait_for(
            "the request body to complete at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true),
        )
        .await?;
    let snapshot = probe.snapshot().await?;
    require_single_attempt(&snapshot, &trace)?;
    if snapshot.req_body_total(&trace) == 0 {
        return Err(format!(
            "no request-body byte reached the upstream, so the framing assertion above is about an empty \
             request\n{snapshot}"
        ));
    }

    Ok(format!(
        "{} upstream, streamed={}: cl={:?} te={:?} headers_eos={:?}, {} body bytes",
        head.listener,
        combo.streamed,
        head.content_length,
        head.transfer_encoding,
        head.headers_eos,
        snapshot.req_body_total(&trace)
    ))
}

async fn shape_bodyless_streamed_coercion(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let out = drive(
        &ctx,
        combo,
        "GET",
        "/bodyless",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::Bodyless,
        Duration::from_secs(10),
    )
    .await?;
    if out.status != Some(200) {
        return Err(format!("expected 200 for the bodyless request, {}", out.describe()));
    }

    let head = head_for(&probe, &trace, combo).await?;
    match combo.up {
        Proto::H1 => {
            let te = head.transfer_encoding.as_deref().unwrap_or("");
            if te.to_ascii_lowercase().contains("chunked") {
                return Err(format!(
                    "a bodyless streamed request must be coerced back to Ordinary, not re-framed as chunked \
                     with a `0\\r\\n\\r\\n` terminator (contract §3 names this the smuggling primitive): {head}"
                ));
            }
            if head.content_length.unwrap_or(0) != 0 {
                return Err(format!(
                    "a bodyless request must not acquire a non-zero Content-Length: {head}"
                ));
            }
        }
        Proto::H2 => {
            if head.transfer_encoding.is_some() {
                return Err(format!("HTTP/2 must never carry Transfer-Encoding: {head}"));
            }
            if head.content_length.unwrap_or(0) != 0 {
                return Err(format!(
                    "a bodyless request must not acquire a non-zero Content-Length: {head}"
                ));
            }
        }
    }

    let snapshot = probe.snapshot().await?;
    require_single_attempt(&snapshot, &trace)?;
    if snapshot.req_body_total(&trace) != 0 {
        return Err(format!(
            "a bodyless request delivered {} body bytes to the upstream\n{snapshot}",
            snapshot.req_body_total(&trace)
        ));
    }

    Ok(format!(
        "bodyless streamed request stayed Ordinary on the {} upstream: cl={:?} te={:?} headers_eos={:?}",
        head.listener, head.content_length, head.transfer_encoding, head.headers_eos
    ))
}

/// The malformed-framing family, HTTP/1 upstream only.
const MALFORMED_BEHAVIORS: &[&str] = &[
    "malformed-cl-and-te",
    "malformed-chunk-size-overstated",
    "malformed-chunk-size-understated",
    "malformed-chunk-terminator-missing",
    "malformed-chunk-terminator-corrupt",
    "malformed-content-length-overstated",
    "malformed-content-length-understated",
];

async fn shape_upstream_malformed_framing(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let mut notes = Vec::new();
    for (index, behavior) in MALFORMED_BEHAVIORS.iter().enumerate() {
        // The first behavior runs under the probe's **primary** trace, the rest
        // under siblings.
        //
        // Not cosmetic. The wrapper's exercised-upstream guard queries the
        // primary trace only, and `/transport/query/{trace}` answers with that
        // trace's own events plus untagged events on its carriers — a sibling
        // trace's `ReqHeaders` is in neither set. A shape that spends every
        // request on a sibling therefore reports "the upstream never received a
        // request" however much work it did, which is precisely what this shape
        // used to do. Anchoring one iteration on the primary trace lets the
        // guard, and the `asserted_kinds` check behind it, tell the truth about
        // this shape instead of vetoing it.
        let trace = if index == 0 {
            probe.trace_id().to_string()
        } else {
            probe.sibling_trace(&format!("mf{index}"))
        };
        let out = drive(
            &ctx,
            combo,
            "POST",
            "/malformed",
            &ctrl(&[
                ("x-up-behavior", behavior),
                ("x-up-resp-len", "16"),
                ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
            ]),
            BodyPlan::ContentLength(b"malformed-framing-probe".to_vec()),
            Duration::from_secs(8),
        )
        .await?;

        let head = head_for(&probe, &trace, combo).await?;
        require_listener(&head, combo)?;
        // Prove the upstream received the request before asserting how it ended.
        let _ = probe
            .wait_for_trace(
                &trace,
                "the request body to complete at the upstream",
                Duration::from_secs(5),
                {
                    let trace = trace.clone();
                    move |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true)
                },
            )
            .await?;
        let resp = probe
            .wait_for_trace(&trace, "the malformed response head", Duration::from_secs(5), {
                let trace = trace.clone();
                move |e| e.is(KIND_RESP_HEAD) && e.tagged(&trace)
            })
            .await?;

        // The recorded framing proves what actually went on the wire, not what
        // the case intended to send.
        let framing = resp.framing.clone().unwrap_or_default();
        if framing.is_empty() || framing == "h2" {
            return Err(format!(
                "`{behavior}` did not put a malformed HTTP/1 framing on the wire (recorded framing {framing:?}): {resp}"
            ));
        }
        if resp.behavior.as_deref() != Some(*behavior) {
            return Err(format!(
                "the backend applied `{:?}` instead of `{behavior}` — the control header did not survive the \
                 proxy, so this iteration asserts nothing: {resp}",
                resp.behavior
            ));
        }

        // A deterministic downstream outcome: either the gateway surfaced the
        // upstream's framing error, or it passed the response through. What it
        // must never do is hang or produce a second upstream attempt.
        let snapshot = probe.snapshot_of(&trace).await?;
        require_single_attempt(&snapshot, &trace)?;
        if out.status.is_none() && out.errors.is_empty() {
            return Err(format!(
                "`{behavior}` produced neither a downstream status nor a downstream error\n{snapshot}"
            ));
        }
        notes.push(format!("{behavior}=>{:?}/{}", out.status, framing));
    }

    Ok(format!("7 malformed framings, all deterministic: {}", notes.join(", ")))
}

async fn shape_upstream_connection_reuse(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace_a = probe.trace_id().to_string();
    let trace_b = probe.sibling_trace("second");

    let first = drive(
        &ctx,
        combo,
        "POST",
        "/reuse",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&trace_a),
            ),
        ]),
        BodyPlan::ContentLength(b"first".to_vec()),
        Duration::from_secs(10),
    )
    .await?;
    if first.status != Some(200) {
        return Err(format!("first request failed, {}", first.describe()));
    }
    let head_a = head_for(&probe, &trace_a, combo).await?;
    let accepted_before = probe.snapshot().await?.conn_accepted_total(combo.up_listener());

    let second = drive(
        &ctx,
        combo,
        "POST",
        "/reuse",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&trace_b),
            ),
        ]),
        BodyPlan::ContentLength(b"second".to_vec()),
        Duration::from_secs(10),
    )
    .await?;
    if second.status != Some(200) {
        return Err(format!("second request failed, {}", second.describe()));
    }
    let head_b = head_for(&probe, &trace_b, combo).await?;

    // Positive twin — the reuse actually happened.
    if head_b.conn != head_a.conn {
        return Err(format!(
            "expected the second request to reuse {} connection {}, it landed on {}\n{}",
            combo.up_listener(),
            head_a.conn,
            head_b.conn,
            probe.snapshot_of(&trace_b).await?
        ));
    }
    let snapshot_b = probe.snapshot_of(&trace_b).await?;
    let carrier = snapshot_b
        .connections()
        .iter()
        .find(|c| c.listener == combo.up_listener() && c.conn == head_a.conn)
        .ok_or_else(|| format!("no connection summary for conn {}\n{snapshot_b}", head_a.conn))?;
    if !carrier
        .traces
        .contains(&crate::access_log_client::trace_id_for_marker(&trace_a))
        || !carrier
            .traces
            .contains(&crate::access_log_client::trace_id_for_marker(&trace_b))
    {
        return Err(format!(
            "expected both traces on one connection, connection {} carried {:?}\n{snapshot_b}",
            carrier.conn, carrier.traces
        ));
    }

    // Negative half — nothing else was opened. An exact delta, not a bound.
    let accepted_after = snapshot_b.conn_accepted_total(combo.up_listener());
    if accepted_after != accepted_before {
        return Err(format!(
            "the second request opened {} new {} upstream connection(s); a reused connection opens none",
            accepted_after - accepted_before,
            combo.up_listener()
        ));
    }
    probe
        .expect_none("any further upstream connection", Duration::from_millis(400), |e| {
            e.is(KIND_CONN_ACCEPTED)
        })
        .await?;

    Ok(format!(
        "{} connection {} served both traces ({} requests), accept counter unchanged at {}",
        combo.up_listener(),
        carrier.conn,
        carrier.requests,
        accepted_after
    ))
}

async fn shape_unfinished_body_poisons_pool(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace_a = probe.trace_id().to_string();
    let trace_b = probe.sibling_trace("second");

    // A chunked request whose body is deliberately never finished, then aborted.
    // The gateway must not return that upstream connection to the pool.
    {
        let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
        conn.write_head(
            "POST",
            "/poison",
            combo.host(),
            &[
                ("Transfer-Encoding", "chunked"),
                ("x-up-behavior", "echo"),
                (
                    "traceparent",
                    &crate::access_log_client::traceparent_for_marker(&trace_a),
                ),
            ],
        )
        .await?;
        conn.write_chunk(&[b'p'; 64]).await?;
        // Give the gateway time to forward the head and the first chunk.
        let head_a = head_for(&probe, &trace_a, combo).await?;
        let _ = probe
            .wait_for_trace(
                &trace_a,
                "the first body chunk at the upstream",
                Duration::from_secs(5),
                {
                    let trace_a = trace_a.clone();
                    move |e| e.is(KIND_REQ_DATA) && e.tagged(&trace_a) && e.len.unwrap_or(0) > 0
                },
            )
            .await?;
        conn.abort();

        // The connection the poisoned request used.
        let poisoned = head_a.conn;
        let accepted_before = probe.snapshot().await?.conn_accepted_total(LISTENER_H1);

        // Let the gateway notice the downstream abort before the follow-up.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let second = drive(
            &ctx,
            combo,
            "POST",
            "/poison",
            &ctrl(&[
                ("x-up-behavior", "echo"),
                (
                    "traceparent",
                    &crate::access_log_client::traceparent_for_marker(&trace_b),
                ),
            ]),
            BodyPlan::ContentLength(b"follow-up-after-poison".to_vec()),
            Duration::from_secs(10),
        )
        .await?;
        if second.status != Some(200) {
            return Err(format!("the follow-up request failed, {}", second.describe()));
        }
        let head_b = head_for(&probe, &trace_b, combo).await?;
        if head_b.conn == poisoned {
            return Err(format!(
                "the upstream H1 connection {poisoned} carried a request body that never completed, yet the \
                 follow-up was served on it — contract §3 requires such a connection never to be pooled\n{}",
                probe.snapshot_of(&trace_b).await?
            ));
        }
        let accepted_after = probe.snapshot_of(&trace_b).await?.conn_accepted_total(LISTENER_H1);
        if accepted_after != accepted_before + 1 {
            return Err(format!(
                "expected exactly one new H1 upstream connection for the follow-up ({} -> {}), the poisoning \
                 claim is not what was observed",
                accepted_before, accepted_after
            ));
        }

        Ok(format!(
            "poisoned H1 conn {poisoned} was not reused; follow-up served on conn {} after exactly one new \
             accept ({} -> {})",
            head_b.conn, accepted_before, accepted_after
        ))
    }
}

// ===========================================================================
// Shapes — downstream side
// ===========================================================================

async fn shape_client_disconnect_mid_upload(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let control = ctrl(&[
        ("x-up-behavior", "echo"),
        ("x-up-drain", "true"),
        ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
    ]);

    match combo.down {
        Proto::H1 => {
            let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
            let mut head: Vec<(&str, &str)> = control.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            head.push(("Transfer-Encoding", "chunked"));
            conn.write_head("POST", "/client-abort", combo.host(), &head).await?;
            conn.write_chunk(&[b'a'; 48]).await?;
            // The upstream must have seen the partial body before the abort,
            // otherwise there is nothing to assert about how it ended.
            let _ = head_for(&probe, &trace, combo).await?;
            let _ = probe
                .wait_for(
                    "the partial request body at the upstream before the client aborts",
                    Duration::from_secs(5),
                    |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.len.unwrap_or(0) > 0,
                )
                .await?;
            // A real TCP RST, not a graceful close.
            conn.abort();
        }
        Proto::H2 => {
            let mut conn = RawH2Conn::connect(&ctx.target_host, ctx.http_port).await?;
            let uri = format!("http://{}:{}/client-abort", combo.host(), ctx.http_port);
            let head: Vec<(&str, &str)> = control.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let (_response, mut send) = conn.send_request("POST", &uri, &head, false)?;
            send.reserve_capacity(48);
            send.send_data(Bytes::from(vec![b'a'; 48]), false)
                .map_err(|e| format!("DATA write: {e}"))?;
            let _ = head_for(&probe, &trace, combo).await?;
            let _ = probe
                .wait_for(
                    "the partial request body at the upstream before the client resets",
                    Duration::from_secs(5),
                    |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.len.unwrap_or(0) > 0,
                )
                .await?;
            h2_reset(&mut send, h2::Reason::NO_ERROR);
            conn.shutdown();
        }
    }

    let snapshot = probe.snapshot().await?;
    let total = snapshot.req_body_total(&trace);
    if total == 0 {
        return Err(format!(
            "no request-body byte reached the upstream before the client disconnected; the case cannot be \
             asserting anything about a partial upload\n{snapshot}"
        ));
    }
    // The body must be partial, not complete: a completed body would mean the
    // abort came too late to test anything.
    if snapshot
        .find(|e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true))
        .is_some()
    {
        return Err(format!(
            "the request body completed at the upstream despite the client aborting mid-upload\n{snapshot}"
        ));
    }
    // A client abort is benign: it must not make the gateway retry upstream.
    require_single_attempt(&snapshot, &trace)?;
    probe
        .expect_none(
            "a retry attempt after the client aborted",
            Duration::from_millis(500),
            |e| e.is(KIND_REQ_HEADERS) && e.tagged(&trace),
        )
        .await?;

    Ok(format!(
        "client aborted after {total} upstream body bytes; exactly one upstream attempt, no retry"
    ))
}

async fn shape_client_sends_trailers(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let out = drive(
        &ctx,
        combo,
        "POST",
        "/trailers",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ]),
        BodyPlan::ChunkedWithTrailers {
            chunks: chunks_of(2, 24, b't'),
            trailers: vec![("x-client-trailer".to_string(), "present".to_string())],
        },
        Duration::from_secs(10),
    )
    .await?;
    if out.status != Some(200) {
        return Err(format!(
            "a trailer-bearing request must still complete, {}",
            out.describe()
        ));
    }

    let head = head_for(&probe, &trace, combo).await?;
    let _ = probe
        .wait_for(
            "the request body to complete at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true),
        )
        .await?;

    let snapshot = probe.snapshot().await?;
    if snapshot.req_body_total(&trace) == 0 {
        return Err(format!(
            "no body byte reached the upstream, so the trailer claim is about an empty request\n{snapshot}"
        ));
    }
    // Contract §4.2: trailers are detected but never forwarded. Exact count over
    // the whole log, plus a window, so neither half can pass vacuously.
    let forwarded = snapshot.count_matching(|e| e.is(KIND_REQ_TRAILERS) && e.tagged(&trace));
    if forwarded != 0 {
        return Err(format!(
            "request trailers must never be forwarded upstream, but the recorder saw {forwarded}\n{snapshot}"
        ));
    }
    probe
        .expect_none(
            "request trailers forwarded to the upstream",
            Duration::from_millis(400),
            |e| e.is(KIND_REQ_TRAILERS) && e.tagged(&trace),
        )
        .await?;
    if out.header("x-up-req-trailers") != Some("false") {
        return Err(format!(
            "the backend reported x-up-req-trailers={:?}, expected `false`. {}",
            out.header("x-up-req-trailers"),
            out.describe()
        ));
    }
    require_single_attempt(&snapshot, &trace)?;

    Ok(format!(
        "downstream trailers accepted, {} upstream body bytes on {}, zero upstream trailer sections",
        snapshot.req_body_total(&trace),
        head.listener
    ))
}

/// HTTP/1 request pipelining, and what "not supported" has to mean.
///
/// This shape was first written expecting both pipelined requests to be
/// answered in order. They are not, and the failing assertion is what proved
/// it. Determined by driving the gateway by hand — two heads in one write, then
/// read until the connection ends: the first request is answered normally with
/// its own response, and the connection is then closed with a clean FIN. The
/// second request produced no access-log entry and no recorder event at all, so
/// it was never forwarded upstream. A sequential (non-pipelined) second request
/// on the same connection is answered, so this is pipelining specifically, not
/// broken keep-alive.
///
/// That is pingora's deliberate design, not an Edgion decision:
/// `HttpSession::reuse` in `pingora-core/src/protocols/http/v1/server.rs` marks
/// the connection un-reusable when `has_bytes_overread()` — "the pipelined
/// request left unread, and the client can attempt to resend".
///
/// So the claim worth pinning is not ordering, which cannot happen, but the
/// safety property that makes refusing to pipeline sound: the second request is
/// never mis-paired with the first request's response, never smuggled upstream
/// as a request nobody routed, and the client is left able to resend it. Those
/// are exactly the failure modes a half-implemented pipelining path produces,
/// and a regression into any of them would be a request-smuggling bug.
async fn shape_client_pipelines(ctx: TestContext, combo: Combo, probe: Arc<TransportProbe>) -> Result<String, String> {
    let trace_a = probe.trace_id().to_string();
    let trace_b = probe.sibling_trace("pipelined");
    let trace_resent = probe.sibling_trace("resent");

    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    // One write, two heads. Whether pingora sees the second request at all
    // depends on its bytes having been over-read into the first request's
    // buffer, so two separate writes would make this a race with the gateway's
    // read loop instead of a property of the gateway.
    let pipelined = format!(
        "{}{}",
        render_h1_head(
            "GET",
            "/pipeline-a",
            combo.host(),
            &[
                ("x-up-behavior", "echo"),
                ("x-up-resp-len", "8"),
                (
                    "traceparent",
                    &crate::access_log_client::traceparent_for_marker(&trace_a)
                ),
            ],
        ),
        render_h1_head(
            "GET",
            "/pipeline-b",
            combo.host(),
            &[
                ("x-up-behavior", "echo"),
                ("x-up-resp-len", "9"),
                (
                    "traceparent",
                    &crate::access_log_client::traceparent_for_marker(&trace_b)
                ),
            ],
        ),
    );
    conn.write_raw(pipelined.as_bytes()).await?;

    // The first request is answered, and answered with its **own** response:
    // the 8-byte length belongs to `/pipeline-a` and the 9-byte length to
    // `/pipeline-b`, so a mis-paired response fails here rather than passing as
    // "a 200 arrived".
    let first = conn.read_response_head(Duration::from_secs(10)).await?;
    if first.status != 200 {
        return Err(format!(
            "the first pipelined request must still be served normally, got {}\n{first}",
            first.status
        ));
    }
    let first_body = conn.read_body(&first, Duration::from_secs(10)).await?;
    if first_body.len() != 8 {
        return Err(format!(
            "the first response carried {} body bytes, expected the 8 that `/pipeline-a` asked for — \
             this is `/pipeline-b`'s response on `/pipeline-a`'s request",
            first_body.len()
        ));
    }
    let head_a = head_for(&probe, &trace_a, combo).await?;
    if head_a.path.as_deref() != Some("/pipeline-a") {
        return Err(format!(
            "the upstream received {:?} for the first request's trace, expected /pipeline-a: {head_a}",
            head_a.path
        ));
    }

    // The connection is then closed rather than reused, and closed without a
    // second response. Fails both ways: any byte here is a response that must
    // not exist, and an open-but-silent connection is a hang.
    conn.expect_close_without_response(Duration::from_secs(5)).await?;

    // The pipelined request was never forwarded. Exact whole-log count over the
    // sibling trace's own answer, plus a bounded window on the same trace —
    // asking the primary trace would forbid an event its answer can never
    // contain.
    let snapshot_b = probe.snapshot_of(&trace_b).await?;
    let leaked = snapshot_b.count_matching(|e| e.tagged(&trace_b));
    if leaked != 0 {
        return Err(format!(
            "the pipelined second request was left unread downstream, so nothing about it may reach the \
             upstream, but the recorder has {leaked} events for its trace\n{snapshot_b}"
        ));
    }
    probe
        .expect_none_for(
            &trace_b,
            "any upstream event for the request that was pipelined and abandoned",
            Duration::from_millis(400),
            |e| e.tagged(&trace_b),
        )
        .await?;

    // Positive twin, so "never forwarded" is not satisfied by a request that
    // was unroutable all along: the same request succeeds when the client
    // resends it on a fresh connection, which is the recovery pingora's comment
    // promises the client.
    let resent = drive(
        &ctx,
        combo,
        "GET",
        "/pipeline-b",
        &ctrl(&[
            ("x-up-behavior", "echo"),
            ("x-up-resp-len", "9"),
            (
                "traceparent",
                &crate::access_log_client::traceparent_for_marker(&trace_resent),
            ),
        ]),
        BodyPlan::Bodyless,
        Duration::from_secs(10),
    )
    .await?;
    if resent.status != Some(200) || resent.body.len() != 9 {
        return Err(format!(
            "the abandoned request must succeed when the client resends it, {}",
            resent.describe()
        ));
    }
    let head_resent = head_for(&probe, &trace_resent, combo).await?;
    if head_resent.path.as_deref() != Some("/pipeline-b") {
        return Err(format!(
            "the resent request reached the upstream as {:?}, expected /pipeline-b: {head_resent}",
            head_resent.path
        ));
    }

    let snapshot_a = probe.snapshot().await?;
    require_single_attempt(&snapshot_a, &trace_a)?;
    require_single_attempt(&probe.snapshot_of(&trace_resent).await?, &trace_resent)?;

    Ok(format!(
        "pipelined request 2 was never forwarded (0 upstream events for its trace); request 1 answered \
         normally on {} conn {} and the connection was then closed, and request 2 succeeded on resend",
        head_a.listener, head_a.conn
    ))
}

/// `Expect: 100-continue` on a plainly proxied route.
///
/// This shape was first written expecting the gateway to answer `100 Continue`.
/// It does not, and the failing assertion is what proved it. Determined by
/// driving the gateway by hand: head with `Expect: 100-continue` and no body,
/// then wait — nothing whatsoever comes back downstream, while the recorder
/// already shows the request head at the upstream with zero body bytes. The
/// same probe against `transport-streamed-*`, the ExtProc `STREAMED` route,
/// behaves identically, so this is not a property of the ordinary disposition.
///
/// The real contract: Edgion writes `100 Continue` only where a plugin buffers
/// the request body. `capture_request_body`
/// (`edgion-gateway/src/plugins/runtime/session_adapter.rs`) writes the interim
/// and then strips `Expect` so the upstream cannot produce a second one — and
/// that is the path `EdgionPlugins/Dsl`'s
/// `body_expect_continue_single_interim_then_success` covers. On a route with
/// no such plugin nothing in the gateway answers the expectation. This shape's
/// `echo` origin emits no informational response, so no interim reaches the
/// downstream client by either route. Informational heads that an HTTP/1 origin
/// does emit are relayed; `HTTPRoute/Backend/InformationalRetry` covers that
/// separate path.
///
/// What this shape pins is therefore that the unanswered expectation is
/// *harmless*, which is the part that could regress into something worse than
/// "no interim": the head is forwarded immediately rather than the request
/// being held hostage waiting for a handshake that will never happen, no body
/// byte is fabricated or leaked upstream before the client sends one, and a
/// client that stops waiting and sends the body anyway (RFC 9110 §10.1.1 — a
/// client "SHOULD NOT wait for an indefinite period") gets a normal 200 with
/// the whole body delivered.
async fn shape_expect_100_continue(
    ctx: TestContext,
    combo: Combo,
    probe: Arc<TransportProbe>,
) -> Result<String, String> {
    let trace = probe.trace_id().to_string();
    let body = b"expect-continue-body".to_vec();
    let length = body.len().to_string();

    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    conn.write_head(
        "POST",
        "/expect",
        combo.host(),
        &[
            ("Content-Length", &length),
            ("Expect", "100-continue"),
            ("x-up-behavior", "echo"),
            ("traceparent", &crate::access_log_client::traceparent_for_marker(&trace)),
        ],
    )
    .await?;

    // The negative claim, as a bounded window on the downstream byte stream:
    // while the client withholds the body, nothing at all is written back — no
    // interim, and no premature final response either.
    conn.expect_silence(Duration::from_secs(1)).await?;

    // The positive twin, so that silence is not the silence of a request that
    // went nowhere: the head is already at the upstream, complete with the
    // Content-Length the client declared. This is also the ordering claim —
    // `head_for` runs before a single body byte is written, so a gateway that
    // held the head back until the body arrived would fail here.
    let up_head = head_for(&probe, &trace, combo).await?;
    if up_head.content_length != Some(body.len() as u64) {
        return Err(format!(
            "the upstream head declared Content-Length {:?}, expected {}: {up_head}",
            up_head.content_length,
            body.len()
        ));
    }

    // Exact count over the whole log: not one body byte reached the upstream
    // while the client was still waiting.
    let snapshot_before = probe.snapshot().await?;
    let early = snapshot_before.req_body_total(&trace);
    if early != 0 {
        return Err(format!(
            "the upstream received {early} body bytes before the client had written any\n{snapshot_before}"
        ));
    }

    // The client gives up waiting and sends the body anyway.
    conn.write_raw(&body).await?;
    let head = conn.read_response_head(Duration::from_secs(10)).await?;
    if head.is_interim() {
        return Err(format!(
            "an interim response arrived after the body — the gateway must not answer an expectation it \
             already declined to answer\n{head}"
        ));
    }
    if head.status != 200 {
        return Err(format!("expected 200 after the body, got {}\n{head}", head.status));
    }
    let _ = conn.read_body(&head, Duration::from_secs(10)).await?;

    let _ = probe
        .wait_for(
            "the request body to complete at the upstream",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(&trace) && e.end_stream == Some(true),
        )
        .await?;
    let snapshot = probe.snapshot().await?;
    if snapshot.req_body_total(&trace) != body.len() {
        return Err(format!(
            "the upstream received {} of {} body bytes\n{snapshot}",
            snapshot.req_body_total(&trace),
            body.len()
        ));
    }
    require_single_attempt(&snapshot, &trace)?;

    // Closing the count: the 200 was the only response head of the exchange, so
    // no interim was written late either.
    conn.expect_silence(Duration::from_millis(300)).await?;

    Ok(format!(
        "no interim for an unbuffered route; head reached the {} upstream with 0 body bytes, then {} body \
         bytes and a 200 once the client sent it anyway",
        up_head.listener,
        body.len()
    ))
}

// ===========================================================================
// Guards
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_case_count_guard_matches_the_declared_matrix() {
        assert_eq!(
            expected_case_count(),
            EXPECTED_TRANSPORT_CASES,
            "EXPECTED_TRANSPORT_CASES must track the shape/combination matrix"
        );
    }

    #[test]
    fn the_suite_produces_the_expected_number_of_uniquely_named_cases() {
        let cases = TransportTestSuite.test_cases();
        assert_eq!(cases.len(), EXPECTED_TRANSPORT_CASES);
        let names: HashSet<&str> = cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names.len(),
            cases.len(),
            "two cases share a name, so one of them is invisible in the report"
        );
    }

    /// A test named for a transport event must declare that it observes it.
    ///
    /// The runtime half of this rule lives in `run_transport_case`, which fails
    /// a case whose declared kinds are absent from the recorder. This half
    /// catches the earlier mistake: declaring nothing relevant in the first
    /// place.
    #[test]
    fn every_shape_name_is_backed_by_the_kinds_it_declares() {
        for shape in SHAPES {
            for combo in shape.combos {
                let kinds = (shape.asserted_kinds)(*combo);
                assert!(
                    kinds.contains(&KIND_REQ_HEADERS),
                    "shape `{}` must at minimum declare ReqHeaders — without it the exercised-upstream \
                     guard is the only thing standing between the case and vacuity",
                    shape.name
                );
                let name = shape.name;
                if name.contains("reset") {
                    assert!(
                        kinds.contains(&KIND_PEER_RESET) || kinds.contains(&KIND_RESP_DONE),
                        "shape `{name}` is named for a reset but declares neither PeerReset (the gateway \
                         resetting us) nor RespDone (the backend's own reset record)"
                    );
                }
                if name.contains("half_close") || name.contains("disconnect") {
                    assert!(
                        kinds.contains(&KIND_PEER_HALF_CLOSE)
                            || kinds.contains(&KIND_RESP_DONE)
                            || kinds.contains(&KIND_REQ_DATA),
                        "shape `{name}` is named for a disconnect but declares nothing that could observe one"
                    );
                }
                if name.contains("trailer") {
                    assert!(
                        kinds.contains(&KIND_REQ_TRAILERS) || shape.absent_kinds.contains(&KIND_REQ_TRAILERS),
                        "shape `{name}` is named for trailers but neither asserts a ReqTrailers event nor \
                         declares that it asserts their absence"
                    );
                }
                if name.contains("framing") || name.contains("bodyless") {
                    assert!(
                        kinds.contains(&KIND_REQ_HEADERS),
                        "shape `{name}` is named for framing but never looks at ReqHeaders"
                    );
                }
            }
        }
    }

    #[test]
    fn a_skipped_combination_is_never_also_a_covered_one() {
        for shape in SHAPES {
            for (combo, reason) in shape.skips {
                assert!(
                    !shape.combos.contains(combo),
                    "shape `{}` both runs and skips {}",
                    shape.name,
                    combo.suffix()
                );
                assert!(
                    reason.len() > 40,
                    "shape `{}` skips {} without a real reason",
                    shape.name,
                    combo.suffix()
                );
            }
        }
    }

    #[test]
    fn every_combination_maps_to_a_hostname_that_exists_in_the_fixtures() {
        let fixtures = concat!(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integration/conf/HTTPRoute/Transport/05_HTTPRoute_transport-h1.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integration/conf/HTTPRoute/Transport/06_HTTPRoute_transport-h2.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integration/conf/HTTPRoute/Transport/08_HTTPRoute_transport-streamed-h1.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integration/conf/HTTPRoute/Transport/09_HTTPRoute_transport-streamed-h2.yaml"
            )),
        );
        for combo in Combo::ALL {
            assert!(
                fixtures.contains(combo.host()),
                "combination {} routes to `{}`, which no Transport fixture declares — every case for it \
                 would hit an unmatched route and assert nothing",
                combo.suffix(),
                combo.host()
            );
        }
    }
}
