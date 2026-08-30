// Frame-level proof that the H2 backend puts the shapes it claims on the wire.
//
// Why raw frames and not an `h2` client: the two shapes this module proves are
// *about* frame identity and frame order — "a complete response, THEN
// RST_STREAM" versus "a reset that destroyed the response". An `h2` client
// cannot answer that question honestly, because `h2` overwrites the stream
// state when it receives the reset: the END_STREAM flag that arrived on the
// preceding DATA frame stops being observable through its API (this is the very
// defect the fork's `h2_watched_*` tests exist for). Reading 9-byte frame
// headers off a `TcpStream` has no such blind spot.
//
// The client here therefore speaks HTTP/2 by hand: connection preface, one
// SETTINGS frame, a HEADERS frame whose header block is HPACK "literal without
// indexing, new name" (§6.2.2 — no dynamic table, no Huffman, so the encoder is
// six lines), and DATA frames that deliberately never carry END_STREAM.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::super::recorder;
use super::handle_conn;

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

const T_DATA: u8 = 0x0;
const T_HEADERS: u8 = 0x1;
const T_RST_STREAM: u8 = 0x3;
const T_SETTINGS: u8 = 0x4;
const T_PING: u8 = 0x6;
const T_GOAWAY: u8 = 0x7;
const T_WINDOW_UPDATE: u8 = 0x8;

const F_ACK: u8 = 0x1;
const F_END_STREAM: u8 = 0x1;
const F_END_HEADERS: u8 = 0x4;

/// How many independent runs each proof performs. The reviewer's own
/// measurement of the broken sequence used 960 runs and found zero good
/// outcomes; a defect with a per-run coin flip is caught by far fewer, and this
/// number keeps `cargo test` honest without making it slow.
const PROOF_RUNS: usize = 40;

fn put_frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    let len = payload.len();
    out.push((len >> 16) as u8);
    out.push((len >> 8) as u8);
    out.push(len as u8);
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
}

/// HPACK §6.2.2 literal header field without indexing, new name, no Huffman.
/// Names and values are asserted short so the 7-bit length prefix is enough.
fn put_header(out: &mut Vec<u8>, name: &str, value: &str) {
    assert!(
        name.len() < 127 && value.len() < 127,
        "header too long for the 7-bit prefix"
    );
    out.push(0x00);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

struct Frame {
    kind: u8,
    flags: u8,
    stream: u32,
    payload: Vec<u8>,
}

impl Frame {
    /// How this frame is rendered into the observed sequence a proof asserts on.
    fn render(&self) -> String {
        match self.kind {
            T_HEADERS => format!("HEADERS(end_stream={})", self.flags & F_END_STREAM != 0),
            T_DATA => format!(
                "DATA(len={},end_stream={})",
                self.payload.len(),
                self.flags & F_END_STREAM != 0
            ),
            T_RST_STREAM => {
                let code = u32::from_be_bytes([self.payload[0], self.payload[1], self.payload[2], self.payload[3]]);
                format!("RST_STREAM({})", super::reason_name(h2::Reason::from(code)))
            }
            other => format!("frame(type={other})"),
        }
    }
}

async fn read_frame(sock: &mut TcpStream) -> Result<Frame, String> {
    let mut head = [0u8; 9];
    sock.read_exact(&mut head)
        .await
        .map_err(|e| format!("frame header read: {e}"))?;
    let len = ((head[0] as usize) << 16) | ((head[1] as usize) << 8) | head[2] as usize;
    let mut payload = vec![0u8; len];
    sock.read_exact(&mut payload)
        .await
        .map_err(|e| format!("frame payload read: {e}"))?;
    Ok(Frame {
        kind: head[3],
        flags: head[4],
        stream: u32::from_be_bytes([head[5] & 0x7f, head[6], head[7], head[8]]),
        payload,
    })
}

/// Start the real backend connection handler on an ephemeral port.
async fn start_backend() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let _ = tcp.set_nodelay(true);
            let conn = recorder::claim_conn(recorder::LISTENER_H2);
            tokio::spawn(async move {
                let _ = handle_conn(tcp, conn).await;
                recorder::mark_closed(recorder::LISTENER_H2, conn);
            });
        }
    });
    addr
}

/// Drive one request through the real backend and return the frame sequence the
/// backend actually put on the wire for stream 1.
///
/// `end_request` decides the premise: `false` leaves the request body open, so
/// the backend's stream is `HalfClosedLocal` when it resets — the RFC 9113 §8.1
/// shape. `true` completes the request body first, which is the premise `echo`
/// used to have and which `h2` answers by suppressing the reset entirely.
async fn run_once(
    addr: std::net::SocketAddr,
    control: &[(&str, &str)],
    end_request: bool,
    quiet_window: Duration,
) -> Result<Vec<String>, String> {
    let mut sock = TcpStream::connect(addr).await.map_err(|e| format!("connect: {e}"))?;
    sock.set_nodelay(true).ok();

    let mut block = Vec::new();
    put_header(&mut block, ":method", "POST");
    put_header(&mut block, ":scheme", "http");
    put_header(&mut block, ":path", "/wire-proof");
    put_header(&mut block, ":authority", "wire.proof");
    for (name, value) in control {
        put_header(&mut block, name, value);
    }

    let mut out = Vec::new();
    out.extend_from_slice(PREFACE);
    put_frame(&mut out, T_SETTINGS, 0, 0, &[]);
    put_frame(&mut out, T_HEADERS, F_END_HEADERS, 1, &block);
    // Enough body to satisfy any `x-up-at-bytes` these proofs use, in one frame.
    put_frame(
        &mut out,
        T_DATA,
        if end_request { F_END_STREAM } else { 0 },
        1,
        &[b'u'; 64],
    );
    sock.write_all(&out).await.map_err(|e| format!("write: {e}"))?;
    sock.flush().await.map_err(|e| format!("flush: {e}"))?;

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + quiet_window;
    loop {
        let frame = match tokio::time::timeout_at(deadline, read_frame(&mut sock)).await {
            // A timeout is an answer, not a failure: it means the backend put
            // nothing more on the wire, which is exactly what a suppressed reset
            // looks like. The caller asserts on the sequence either way.
            Err(_) => break,
            Ok(Ok(frame)) => frame,
            Ok(Err(_)) => break,
        };
        match frame.kind {
            T_SETTINGS if frame.flags & F_ACK == 0 => {
                let mut ack = Vec::new();
                put_frame(&mut ack, T_SETTINGS, F_ACK, 0, &[]);
                sock.write_all(&ack).await.map_err(|e| format!("settings ack: {e}"))?;
            }
            T_PING if frame.flags & F_ACK == 0 => {
                let mut ack = Vec::new();
                put_frame(&mut ack, T_PING, F_ACK, 0, &frame.payload);
                sock.write_all(&ack).await.map_err(|e| format!("ping ack: {e}"))?;
            }
            T_SETTINGS | T_PING | T_WINDOW_UPDATE => {}
            T_GOAWAY => break,
            _ => {
                if frame.stream == 1 {
                    let done = frame.kind == T_RST_STREAM;
                    seen.push(frame.render());
                    if done {
                        break;
                    }
                }
            }
        }
    }
    Ok(seen)
}

/// THE proof this module exists for: a complete response **followed by**
/// RST_STREAM(NO_ERROR), on the wire, in that order, every single run.
///
/// Both halves of the old defect are excluded by the assertion:
///   * a reset that ran before the queue flushed destroys the HEADERS and DATA
///     frames, so the sequence would be `RST_STREAM` alone;
///   * a reset `h2` suppresses (`is_closed && is_empty`) leaves the sequence at
///     `HEADERS, DATA` with no reset at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_complete_response_then_rst_stream_no_error_is_what_reaches_the_wire() {
    let addr = start_backend().await;
    let expected = vec![
        "HEADERS(end_stream=false)".to_string(),
        "DATA(len=16,end_stream=true)".to_string(),
        "RST_STREAM(NO_ERROR)".to_string(),
    ];

    let mut outcomes: Vec<Vec<String>> = Vec::new();
    for _ in 0..PROOF_RUNS {
        outcomes.push(
            run_once(
                addr,
                &[
                    ("x-up-behavior", "early-response"),
                    ("x-up-at-bytes", "8"),
                    ("x-up-drain", "false"),
                    ("x-up-after-response", "reset"),
                    ("x-up-reset-code", "NO_ERROR"),
                ],
                false,
                Duration::from_secs(3),
            )
            .await
            .expect("the wire-proof client failed"),
        );
    }

    let good = outcomes.iter().filter(|o| **o == expected).count();
    assert_eq!(
        good,
        PROOF_RUNS,
        "the intended wire shape must occur on every run; got {good}/{PROOF_RUNS}. \
         Distinct observed sequences: {:?}",
        distinct(&outcomes)
    );
}

/// The premise guard. `echo` — drain the whole request body, then reset — is the
/// shape this backend used to run, and it cannot express RFC 9113 §8.1 at all:
/// once the peer's END_STREAM has been received the stream is closed, and `h2`
/// answers `send_reset` by emitting nothing. Pinning that here is what stops the
/// integration case from quietly drifting back onto a premise that never resets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn draining_the_request_body_first_makes_h2_suppress_the_reset_entirely() {
    let addr = start_backend().await;
    // Every run here has to sit out a quiet window to prove a *negative* — that
    // no reset frame follows — so the runs are driven concurrently, on separate
    // connections, rather than paying that window forty times over.
    let mut runs = Vec::new();
    for _ in 0..PROOF_RUNS {
        runs.push(tokio::spawn(async move {
            run_once(
                addr,
                &[
                    ("x-up-behavior", "echo"),
                    ("x-up-after-response", "reset"),
                    ("x-up-reset-code", "NO_ERROR"),
                ],
                true,
                // Comfortably longer than RESET_FLUSH_GRACE: if a reset were
                // ever going to be emitted, it would have arrived by now.
                Duration::from_millis(600),
            )
            .await
        }));
    }
    let mut outcomes: Vec<Vec<String>> = Vec::new();
    for run in runs {
        outcomes.push(
            run.await
                .expect("wire-proof run panicked")
                .expect("the wire-proof client failed"),
        );
    }

    let with_reset = outcomes
        .iter()
        .filter(|o| o.iter().any(|f| f.starts_with("RST_STREAM")))
        .count();
    assert_eq!(
        with_reset,
        0,
        "a drained request body leaves the stream closed, so no RST_STREAM can reach the wire; \
         {with_reset}/{PROOF_RUNS} runs carried one. Distinct observed sequences: {:?}",
        distinct(&outcomes)
    );
    let complete = outcomes
        .iter()
        .filter(|o| {
            **o == vec![
                "HEADERS(end_stream=false)".to_string(),
                "DATA(len=16,end_stream=true)".to_string(),
            ]
        })
        .count();
    assert_eq!(
        complete,
        PROOF_RUNS,
        "the response itself must still be complete on every run; got {complete}/{PROOF_RUNS}. \
         Distinct observed sequences: {:?}",
        distinct(&outcomes)
    );
}

/// The recorder half of the same claim.
///
/// The old backend recorded `RespDone code=NO_ERROR` *before* calling
/// `send_reset`, so the event said "a reset was sent" on every run — including
/// the runs where `h2` sent nothing at all. An anti-vacuity guard reading that
/// log could not tell the two apart. This pins the recorded claim to the wire in
/// both directions: exactly one reset claim when the frame is really there, and
/// exactly zero when `h2` swallows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recorder_claims_a_reset_code_only_when_one_reached_the_wire() {
    let addr = start_backend().await;

    let emitted = run_once(
        addr,
        &[
            ("x-up-behavior", "early-response"),
            ("x-up-at-bytes", "8"),
            ("x-up-drain", "false"),
            ("x-up-after-response", "reset"),
            ("x-up-reset-code", "NO_ERROR"),
            ("x-trace-id", "wire-proof-open"),
        ],
        false,
        Duration::from_secs(3),
    )
    .await
    .expect("the wire-proof client failed");
    assert!(
        emitted.iter().any(|f| f == "RST_STREAM(NO_ERROR)"),
        "premise: this run must actually carry a reset, got {emitted:?}"
    );
    assert_eq!(
        reset_claims("wire-proof-open"),
        1,
        "the recorder must claim exactly one NO_ERROR reset for a run that carried one; log: {}",
        recorder::query("wire-proof-open")
    );

    let suppressed = run_once(
        addr,
        &[
            ("x-up-behavior", "echo"),
            ("x-up-after-response", "reset"),
            ("x-up-reset-code", "NO_ERROR"),
            ("x-trace-id", "wire-proof-ended"),
        ],
        true,
        Duration::from_millis(600),
    )
    .await
    .expect("the wire-proof client failed");
    assert!(
        !suppressed.iter().any(|f| f.starts_with("RST_STREAM")),
        "premise: h2 must have suppressed the reset on this run, got {suppressed:?}"
    );
    assert_eq!(
        reset_claims("wire-proof-ended"),
        0,
        "a run where h2 emitted no reset must not leave a reset claim behind; log: {}",
        recorder::query("wire-proof-ended")
    );
}

/// Whole-log count of `RespDone` events claiming a `NO_ERROR` reset for a trace.
/// An exact count, never a bound: `<= 1` is also satisfied by zero.
fn reset_claims(trace: &str) -> usize {
    recorder::query(trace)["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == recorder::KIND_RESP_DONE && e["trace_id"] == trace && e["code"] == "NO_ERROR")
        .count()
}

fn distinct(outcomes: &[Vec<String>]) -> Vec<(Vec<String>, usize)> {
    let mut out: Vec<(Vec<String>, usize)> = Vec::new();
    for o in outcomes {
        match out.iter_mut().find(|(k, _)| k == o) {
            Some((_, n)) => *n += 1,
            None => out.push((o.clone(), 1)),
        }
    }
    out
}
