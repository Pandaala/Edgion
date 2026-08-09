# Canonical Pingora Request-Body Transport Interface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the canonical request-body transport interface (terminal action, transport facts, framing disposition, retry predicate) in the Pingora fork as five independently green commits.

**Architecture:** Five commits per design §7 of
`05-pingora-canonical-interface-design.md`: (1) response-committed retry
guard, (2) transport facts in `pingora-core`, (3) upstream body disposition,
(4) the typed terminate channel with `select!`-driven sibling cancellation,
(5) the `request_retry_allowed` predicate. The archived prototype
`04-deferred-implementation/pingora.patch` (same base commit) is the source
for most code; behavior is ported, with the canonical deltas from design §8.

**Tech Stack:** Rust, tokio, pingora fork at `/Users/caohao/ws2/pingora`,
branch `edgion`, base `b829a34`. No new dependencies.

## Global Constraints

- Repository: `/Users/caohao/ws2/pingora`, branch `edgion`, clean at `b829a34`. All line numbers cited below are exact at this commit.
- All file content in English (identifiers, comments, tests, commit messages).
- No new crate dependencies; no feature flags added.
- Every commit must independently pass: `cargo test -p pingora-core --lib`, `cargo test -p pingora-proxy --lib`, `cargo test -p pingora-proxy --test test_request_body_seam` (once that file exists), `cargo fmt --all -- --check`.
- The new integration test file must NOT use `mod utils;` — the existing harness requires a local openresty; all upstreams in the new file are self-contained tokio TCP listeners.
- End every commit message with the trailer line: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- The prototype patch is at `/Users/caohao/ws2/Edgion/tasks/archive/completed-tasks/request-body-capability-followups/08-ext-proc-streamed-request-body/04-deferred-implementation/pingora.patch` (referred to below as "the prototype patch"). It applies cleanly to the current base; individual hunks are quoted inline where used.

---

### Task 1: Response-committed retry guard (commit 1, code)

**Files:**
- Modify: `pingora-proxy/src/lib.rs:876-882` (the retry loop) and add one helper + test module near the end of the file.

**Interfaces:**
- Produces: `fn final_response_committed(resp: Option<&ResponseHeader>) -> bool` (private, `pingora-proxy/src/lib.rs`). Task 2's integration test exercises it end-to-end.

- [ ] **Step 1: Write the failing unit tests**

Add at the end of `pingora-proxy/src/lib.rs`:

```rust
#[cfg(test)]
mod retry_guard_tests {
    use super::*;

    fn resp(code: u16) -> ResponseHeader {
        ResponseHeader::build(code, None).unwrap()
    }

    #[test]
    fn no_committed_response_permits_retry() {
        assert!(!final_response_committed(None));
    }

    #[test]
    fn retry_guard_ignores_1xx_but_not_101() {
        // H1 can store a 1xx informational response in response_written();
        // it is not final and must not block a retry.
        assert!(!final_response_committed(Some(&resp(100))));
        assert!(!final_response_committed(Some(&resp(103))));
        // 101 is a final response despite being 1xx.
        assert!(final_response_committed(Some(&resp(101))));
    }

    #[test]
    fn final_statuses_forbid_retry() {
        assert!(final_response_committed(Some(&resp(200))));
        assert!(final_response_committed(Some(&resp(502))));
    }
}
```

`ResponseHeader` is already in scope in `lib.rs` (see the `BAD_GATEWAY`
static around line 727).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p pingora-proxy --lib retry_guard`
Expected: FAIL — `final_response_committed` not found.

- [ ] **Step 3: Implement the helper and wire it into the retry loop**

Add above the `retry_guard_tests` module:

```rust
/// Whether a final downstream response has been committed, which forbids any
/// further upstream retry: the client has already received bytes from this
/// attempt, and a retry would concatenate two upstream responses into one
/// downstream body.
///
/// A 1xx informational response is not final — the H1 downstream session can
/// store one in `response_written()` — except 101, which switches protocols
/// and is final. The H2 session never stores a 1xx, so the informational
/// check is inert there.
fn final_response_committed(resp: Option<&ResponseHeader>) -> bool {
    resp.is_some_and(|r| {
        !r.status.is_informational() || r.status == http::StatusCode::SWITCHING_PROTOCOLS
    })
}
```

In the retry loop, change `lib.rs:876-879` from:

```rust
            match e {
                Some(error) => {
                    let retry = error.retry();
                    proxy_error = Some(error);
```

to:

```rust
            match e {
                Some(mut error) => {
                    if final_response_committed(session.response_written()) {
                        error.retry = false.into();
                    }
                    let retry = error.retry();
                    proxy_error = Some(error);
```

- [ ] **Step 4: Run the unit tests and the full proxy lib suite**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS (all pre-existing tests plus the three new ones).

Do not commit yet — commit 1 includes Task 2's integration test.

---

### Task 2: Integration scaffold + reused-connection no-retry-after-commit test (commit 1, test + commit)

**Files:**
- Create: `pingora-proxy/tests/test_request_body_seam.rs`

**Interfaces:**
- Produces: the `SeamProxy` test service (H1 on `127.0.0.1:6191`, h2c on `127.0.0.1:6192`, `max_retries = 2`), the `spawn_scripted_upstream` helper, and the `init()` starter. Tasks 8, 13, and 14 extend this file and rely on these exact names.

- [ ] **Step 1: Write the scaffold and the failing test**

Create `pingora-proxy/tests/test_request_body_seam.rs` with this full content:

```rust
// Self-contained integration tests for the canonical request-body transport
// interface. Deliberately does NOT use tests/utils: that harness needs a
// local openresty. Every upstream here is a scripted tokio TCP listener.

use async_trait::async_trait;
use bytes::Bytes;
use once_cell::sync::Lazy;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_core::services::ServiceWithDependents;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::Result;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Builder, Runtime};

pub static RT: Lazy<Runtime> =
    Lazy::new(|| Builder::new_multi_thread().enable_all().build().unwrap());

#[derive(Default)]
pub struct SeamCtx {
    body_bytes_seen: usize,
}

pub struct SeamProxy {}

#[async_trait]
impl ProxyHttp for SeamProxy {
    type CTX = SeamCtx;
    fn new_ctx(&self) -> Self::CTX {
        SeamCtx::default()
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let port = session
            .req_header()
            .headers
            .get("x-port")
            .and_then(|v| v.to_str().ok())
            .expect("tests must set x-port")
            .to_string();
        Ok(Box::new(HttpPeer::new(
            format!("127.0.0.1:{port}"),
            false,
            "".to_string(),
        )))
    }
}

fn start_seam_server() {
    thread::spawn(|| {
        let mut conf = ServerConf::default();
        conf.max_retries = 2;
        let conf = Arc::new(conf);

        let mut server = Server::new(None).unwrap();
        server.bootstrap();

        let mut h1 = pingora_proxy::http_proxy_service(&conf, SeamProxy {});
        h1.add_tcp("127.0.0.1:6191");

        let mut h2c = pingora_proxy::http_proxy_service(&conf, SeamProxy {});
        let logic = h2c.app_logic_mut().unwrap();
        let mut opts = pingora_core::apps::HttpServerOptions::default();
        opts.h2c = true;
        logic.server_options = Some(opts);
        h2c.add_tcp("127.0.0.1:6192");

        let services: Vec<Box<dyn ServiceWithDependents>> =
            vec![Box::new(h1), Box::new(h2c)];
        server.add_services(services);
        server.run_forever();
    });
    // Wait until the listeners accept.
    std::thread::sleep(Duration::from_millis(500));
}

static SEAM_SERVER: Lazy<()> = Lazy::new(start_seam_server);

pub fn init() {
    Lazy::force(&SEAM_SERVER);
}

/// A scripted H1 upstream. For each accepted connection it serves requests
/// sequentially; the k-th request overall (0-based, across connections) is
/// answered with `script[k]`, then the behavior in the script entry runs.
/// Returns (port, served-request counter).
pub enum UpstreamStep {
    /// Read one request, write this exact byte string, keep the connection.
    Respond(&'static [u8]),
    /// Read one request, write this exact byte string, then close the
    /// connection immediately.
    RespondThenClose(&'static [u8]),
    /// Read one request, then close without writing anything.
    CloseWithoutResponse,
    /// Read one request (headers only), then hang forever.
    Hang,
}

pub fn spawn_scripted_upstream(
    script: Vec<UpstreamStep>,
) -> (u16, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_ret = counter.clone();
    let listener = RT.block_on(async { TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let port = listener.local_addr().unwrap().port();
    let script = Arc::new(script);

    RT.spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let counter = counter.clone();
            let script = script.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    // Read until end of request headers.
                    while !contains_header_end(&pending) {
                        let n = match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        pending.extend_from_slice(&buf[..n]);
                    }
                    // This scripted upstream only receives GET requests or
                    // requests whose body it is allowed to ignore; drain
                    // whatever arrived alongside the headers.
                    pending.clear();

                    let idx = counter.fetch_add(1, Ordering::SeqCst);
                    match script.get(idx) {
                        Some(UpstreamStep::Respond(bytes)) => {
                            if stream.write_all(bytes).await.is_err() {
                                return;
                            }
                        }
                        Some(UpstreamStep::RespondThenClose(bytes)) => {
                            let _ = stream.write_all(bytes).await;
                            return;
                        }
                        Some(UpstreamStep::CloseWithoutResponse) => return,
                        Some(UpstreamStep::Hang) => {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                            return;
                        }
                        None => return,
                    }
                }
            });
        }
    });
    (port, counter_ret)
}

fn contains_header_end(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

const OK_KEEPALIVE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok";

#[test]
fn no_retry_after_response_header_committed_on_reused_conn() {
    init();
    // Request 1 succeeds and pools the upstream connection. Request 2 reuses
    // it; the upstream commits a 200 with Content-Length: 100, sends a
    // partial body, and closes. The resulting upstream read error is
    // retryable (reused connection), but the response is already committed
    // downstream — the guard must prevent a second upstream attempt.
    let (port, counter) = spawn_scripted_upstream(vec![
        UpstreamStep::Respond(OK_KEEPALIVE),
        UpstreamStep::RespondThenClose(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial",
        ),
        // If the guard is broken, a third upstream request arrives here.
        UpstreamStep::Respond(OK_KEEPALIVE),
    ]);

    RT.block_on(async {
        let client = reqwest::Client::new();
        let res = client
            .get("http://127.0.0.1:6191/")
            .header("x-port", port.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.text().await.unwrap(), "ok");

        let res = client
            .get("http://127.0.0.1:6191/")
            .header("x-port", port.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        // The committed-but-truncated body must surface as a body read
        // error, not as a silently concatenated retry response.
        assert!(res.text().await.is_err());
    });

    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "a retry after response commit must not reach the upstream"
    );
}
```

Note: the design names this scenario
`h2_error_no_retry_after_header_sent_on_reused_conn`. This openresty-free
harness exercises the same invariant through the H1 reused-connection path;
the guard itself lives in the protocol-agnostic retry loop, so the coverage
is equivalent. Record any behavioral difference discovered here instead of
silently adapting.

- [ ] **Step 2: Run the test to verify it passes with the guard (and fails without it)**

Run: `cargo test -p pingora-proxy --test test_request_body_seam`
Expected: PASS.

Then verify the test actually guards something: temporarily comment out the
two `error.retry = false.into();` guard lines from Task 1 Step 3, rerun, and
confirm the counter assertion fails (value 3). Restore the lines and rerun to
green. If the counter stays 2 even without the guard, stop and investigate
before committing — the scenario is not exercising the hole.

- [ ] **Step 3: Format and commit**

```bash
cargo fmt --all
git -C /Users/caohao/ws2/pingora add pingora-proxy/src/lib.rs pingora-proxy/tests/test_request_body_seam.rs
git -C /Users/caohao/ws2/pingora commit -m "Never retry after a final downstream response is committed

A retryable upstream error on a reused connection could re-issue the
request after the response header had already been sent downstream,
concatenating two upstream responses into one downstream body. 1xx
informational responses (excluding 101) do not block retries.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: H1 trailer-presence fact (commit 2, part 1)

**Files:**
- Modify: `pingora-core/src/protocols/http/v1/body.rs` (struct at :149, `new` at :160, `reinit` at :171, chunked terminal parse at :722-807, enum at :903)
- Modify: `pingora-core/src/protocols/http/v1/server.rs:582` (after `is_body_empty`)

**Interfaces:**
- Produces: `BodyReader::trailers_present(&self) -> bool`; `HttpSession::request_trailers_present(&mut self) -> bool` and `HttpSession::request_headers_end_stream(&mut self) -> bool` on the v1 server session. Task 5 dispatches to these.

All hunks below are verbatim from the prototype patch (design §2.3 keeps
these names).

- [ ] **Step 1: Add the failing assertions to the three existing chunked tests**

In `pingora-core/src/protocols/http/v1/body.rs`, locate the three existing
tests by their current assertion lines (exact at base `b829a34`) and append
one assertion to each:

- After line 1934 (`assert_eq!(body_reader.get_body_overread(), Some(&b"abc"[..]));`): add `assert!(!body_reader.trailers_present());`
- After line 2007 (`assert_eq!(body_reader.get_body_overread(), None);`, the test whose chunked input contains trailer fields): add `assert!(body_reader.trailers_present());`
- After line 2038 (`assert_eq!(body_reader.get_body_overread(), None);` in the following trailer-bearing test): add `assert!(body_reader.trailers_present());`

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-core --lib v1::body`
Expected: FAIL — `trailers_present` not found.

- [ ] **Step 3: Implement the flag**

In `body.rs`:

1. Add field to `BodyReader` (struct at :149): `trailers_present: bool,`
2. Initialize it in `BodyReader::new` (:160): `trailers_present: false,`
3. Reset it in `reinit` (:171): `self.trailers_present = false;`
4. Add the accessor next to `body_empty` (:257-260):

```rust
    pub fn trailers_present(&self) -> bool {
        self.trailers_present
    }
```

5. Change the enum (:903) from:

```rust
pub enum TrailersEndParseState {
    NotEnd(usize),   // start of bytes after CR or LF bytes
    Complete(usize), // index of message completion
}
```

to:

```rust
pub enum TrailersEndParseState {
    NotEnd(usize),         // start of bytes after CR or LF bytes
    Complete(usize, bool), // index of message completion, whether fields were present
}
```

6. In the parse function (around :795-803), change the completion return from
`Ok(TrailersEndParseState::Complete(next_parse_index))` to:

```rust
                    Ok(TrailersEndParseState::Complete(
                        next_parse_index,
                        trailers_read > 0,
                    ))
```

7. At the consumer (around :729), change the match arm from
`TrailersEndParseState::Complete(end_idx) => {` to
`TrailersEndParseState::Complete(end_idx, trailers_present) => {` and add
`self.trailers_present = trailers_present;` immediately before the existing
`self.finish_body_buf(start + end_idx, n);`.

In `v1/server.rs`, after `is_body_empty` (ends :582):

```rust
    /// Whether the request ended with actual trailer fields.
    ///
    /// This is meaningful after the request body reader reaches EOF. A
    /// `Trailer` declaration header alone does not affect this result.
    pub fn request_trailers_present(&mut self) -> bool {
        self.init_body_reader();
        self.body_reader.trailers_present()
    }

    /// Whether request framing ended at the header section.
    pub fn request_headers_end_stream(&mut self) -> bool {
        self.is_body_empty()
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pingora-core --lib`
Expected: PASS. Do not commit yet.

---

### Task 4: H2 facts and `is_body_done` tightening (commit 2, part 2)

**Files:**
- Modify: `pingora-core/src/protocols/http/v2/server.rs` (struct :106, constructor :174-199, body-read EOF path :251-259, `is_body_done` :611-629, accessors after :636, existing tests :1108-1230, new test after :1230)

**Interfaces:**
- Produces: `HttpSession::request_headers_end_stream(&self) -> bool` and `HttpSession::request_trailers_present(&self) -> bool` on the v2 server session. Task 5 dispatches to these.

All changes are verbatim from the prototype patch.

- [ ] **Step 1: Write the failing test**

Add after the existing test ending at :1230 in
`pingora-core/src/protocols/http/v2/server.rs` (inside `mod test`; extend the
`use` line at :256 of the module — currently
`use http::{HeaderValue, Method, Request};` — with `StatusCode`):

```rust
    #[tokio::test]
    async fn test_request_trailer_presence_is_transport_observed() {
        let (client, server) = duplex(65536);

        let client = tokio::spawn(async move {
            let (h2, connection) = h2::client::handshake(client).await.unwrap();
            tokio::spawn(async move {
                connection.await.unwrap();
            });

            let request = Request::builder()
                .method(Method::POST)
                .uri("https://www.example.com/")
                .header("trailer", "x-checksum")
                .body(())
                .unwrap();
            let (response, mut body) = h2
                .ready()
                .await
                .unwrap()
                .send_request(request, false)
                .unwrap();
            body.send_data("payload".into(), false).unwrap();
            let mut trailers = HeaderMap::new();
            trailers.insert("x-checksum", HeaderValue::from_static("ok"));
            body.send_trailers(trailers).unwrap();
            assert_eq!(response.await.unwrap().status(), StatusCode::NO_CONTENT);
        });

        let mut connection = handshake(Box::new(server), None).await.unwrap();
        let digest = Arc::new(Digest::default());
        let mut handlers = Vec::new();
        while let Some(mut http) = HttpSession::from_h2_conn(&mut connection, digest.clone())
            .await
            .unwrap()
        {
            handlers.push(tokio::spawn(async move {
                assert!(!http.request_headers_end_stream());
                let data = http.read_body_bytes().await.unwrap().unwrap();
                assert_eq!(data, "payload");
                assert!(!http.is_body_done());
                assert!(http.read_body_bytes().await.unwrap().is_none());
                assert!(http.is_body_done());
                assert!(http.request_trailers_present());

                let response =
                    Box::new(ResponseHeader::build(StatusCode::NO_CONTENT, None).unwrap());
                http.write_response_header(response, true).unwrap();
            }));
        }
        client.await.unwrap();
        for handler in handlers {
            handler.await.unwrap();
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-core --lib v2::server`
Expected: FAIL — `request_headers_end_stream` / `request_trailers_present` not found.

- [ ] **Step 3: Implement**

In `v2/server.rs`:

1. Struct (:106): add `request_headers_end_stream: bool,` after
   `request_body_reader: RecvStream,` and `request_trailers_present: bool,`
   after the `early_body_buffer_released: bool,` field (:175).
2. Constructor (:176-199): before building the struct add
   `let request_headers_end_stream = request_body_reader.is_end_stream();`
   and initialize both fields
   (`request_headers_end_stream,` / `request_trailers_present: false,`).
3. Body-read EOF path — replace :251-259, currently:

```rust
        } else if let Some(buffer) = self.early_body_buffer.as_mut() {
            self.early_body_capture_poisoned = true;
            buffer.finish_capture().await?;
            self.early_body_capture_poisoned = false;
        }
```

with:

```rust
        } else {
            let trailers = self.request_body_reader.trailers().await.or_err(
                ErrorType::ReadError,
                "while reading downstream request trailers",
            )?;
            self.request_trailers_present = trailers.is_some_and(|fields| !fields.is_empty());
            if let Some(buffer) = self.early_body_buffer.as_mut() {
                self.early_body_capture_poisoned = true;
                buffer.finish_capture().await?;
                self.early_body_capture_poisoned = false;
            }
        }
```

(`or_err` and `ErrorType` are already used in this file; add imports only if
the compiler asks.)

4. `is_body_done` (:611-629) — replace the final expression
   `self.is_body_empty() || self.request_body_reader.is_end_stream()` with
   `self.request_body_reader.is_end_stream()` and delete the two comment
   lines above it (`// Check no body in request` / `// Also check we hit end
   of stream`). This is the deliberate tightening from design §4.3: a request
   with `Content-Length: 0` but no END_STREAM is not done.
5. Accessors after `is_body_empty` (ends :635):

```rust
    /// Whether the initial HEADERS frame carried END_STREAM.
    pub fn request_headers_end_stream(&self) -> bool {
        self.request_headers_end_stream
    }

    /// Whether actual request trailer fields were received.
    ///
    /// This is meaningful after `read_body_bytes()` returns EOF.
    pub fn request_trailers_present(&self) -> bool {
        self.request_trailers_present
    }
```

6. Fix the two existing tests broken by the tightening, exactly as the
   prototype patch does:
   - Test around :1130: change `assert!(http.is_body_done());` to
     `assert!(!http.request_headers_end_stream());` followed by
     `assert!(!http.is_body_done());`, and change the final
     `assert!(http.read_body_or_idle(http.is_body_done()).await.is_err());`
     to `assert!(http.read_body_or_idle(false).await.is_err());` with the
     comment `// 3. The client drops the response stream after sending its
     request EOS, so this test observes the resulting reset.`
   - Test around :1203: add `assert!(!http.request_headers_end_stream());`
     before the existing `assert!(!http.is_body_empty());`, and change
     `http.read_body_or_idle(http.is_body_done()).await.unwrap();` to
     `let _ = http.read_body_or_idle(false).await;` with the comment
     `// 3. Waiting for the client EOS or the reset caused by the client
     dropping its response stream.`

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pingora-core --lib`
Expected: PASS. Do not commit yet.

---

### Task 5: Session dispatch + commit (commit 2, part 3)

**Files:**
- Modify: `pingora-core/src/protocols/http/server.rs:575` (after the method ending at :577)

**Interfaces:**
- Consumes: the v1/v2 session methods from Tasks 3-4.
- Produces: `Session::request_headers_end_stream(&mut self) -> Option<bool>` and `Session::request_trailers_present(&mut self) -> Option<bool>` on the unified `Session` enum. Commit 4 (Task 11) and Edgion consume these.

- [ ] **Step 1: Implement the dispatch**

In `pingora-core/src/protocols/http/server.rs`, after the method ending at
:577 and before `retry_buffer_truncated` (:583):

```rust
    /// Whether the downstream transport ended the request on its initial
    /// headers. Unsupported session types return `None`.
    pub fn request_headers_end_stream(&mut self) -> Option<bool> {
        match self {
            Self::H1(s) => Some(s.request_headers_end_stream()),
            Self::H2(s) => Some(s.request_headers_end_stream()),
            Self::Subrequest(_) | Self::Custom(_) => None,
        }
    }

    /// Whether the downstream request contained actual trailer fields.
    ///
    /// This is meaningful after a body read reports EOF. Unsupported session
    /// types return `None`.
    pub fn request_trailers_present(&mut self) -> Option<bool> {
        match self {
            Self::H1(s) => Some(s.request_trailers_present()),
            Self::H2(s) => Some(s.request_trailers_present()),
            Self::Subrequest(_) | Self::Custom(_) => None,
        }
    }
```

- [ ] **Step 2: Run the full core suite**

Run: `cargo test -p pingora-core --lib`
Expected: PASS (the Session methods are dispatch-only; the v1/v2 tests from
Tasks 3-4 are their coverage).

- [ ] **Step 3: Verify the proxy crate still builds and passes**

Run: `cargo test -p pingora-proxy --lib && cargo test -p pingora-proxy --test test_request_body_seam`
Expected: PASS.

- [ ] **Step 4: Format and commit**

```bash
cargo fmt --all
git -C /Users/caohao/ws2/pingora add pingora-core/src/protocols/http/v1/body.rs pingora-core/src/protocols/http/v1/server.rs pingora-core/src/protocols/http/v2/server.rs pingora-core/src/protocols/http/server.rs
git -C /Users/caohao/ws2/pingora commit -m "Expose downstream request headers-EOS and trailer-presence facts

H1 records whether the chunked terminal section carried actual trailer
fields; H2 snapshots END_STREAM on HEADERS and polls RecvStream::trailers
after DATA ends. H2 is_body_done now reports the transport EOS fact
instead of inferring completion from Content-Length: 0.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: `UpstreamRequestBodyDisposition` trait surface (commit 3, part 1)

**Files:**
- Modify: `pingora-proxy/src/proxy_trait.rs` (after the imports ending :22, and inside the `ProxyHttp` trait after `request_body_filter`, which ends :125)
- Modify: `pingora-proxy/src/lib.rs:91-98` (re-exports and prelude)

**Interfaces:**
- Produces: `pub enum UpstreamRequestBodyDisposition { Ordinary, Bodyless, Streamed }` and trait method `fn upstream_request_body_disposition(&self, _session: &Session, _ctx: &Self::CTX) -> UpstreamRequestBodyDisposition` (sync, default `Ordinary`). Tasks 7-8 consume both; Edgion consumes them later.

- [ ] **Step 1: Write the failing default-behavior test**

Add at the end of `pingora-proxy/src/proxy_trait.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct DefaultsOnly;

    #[async_trait]
    impl ProxyHttp for DefaultsOnly {
        type CTX = ();

        fn new_ctx(&self) -> Self::CTX {}

        async fn upstream_peer(
            &self,
            _session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<Box<HttpPeer>> {
            unreachable!("upstream_peer is not used by trait-default unit tests")
        }
    }

    #[test]
    fn disposition_defaults_to_ordinary() {
        let io = tokio_test::io::Builder::new().build();
        let session = Session::new_h1(Box::new(io));
        assert_eq!(
            DefaultsOnly.upstream_request_body_disposition(&session, &()),
            UpstreamRequestBodyDisposition::Ordinary
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-proxy --lib proxy_trait`
Expected: FAIL — type and method not found.

- [ ] **Step 3: Implement**

In `proxy_trait.rs`, after the imports (ends :22):

```rust
/// How the proxy should frame the request body sent to an upstream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpstreamRequestBodyDisposition {
    /// Preserve Pingora's ordinary request-body framing behavior.
    #[default]
    Ordinary,
    /// The application guarantees that no upstream request body will follow.
    Bodyless,
    /// The body is streamed and its final length is not known when headers
    /// are sent.
    Streamed,
}
```

Inside the trait, after `request_body_filter` (ends :125):

```rust
    /// Select the upstream request-body framing contract.
    ///
    /// Queried once per upstream attempt, before the upstream request
    /// headers are written. Sync on purpose: `ProxyHttp` is an
    /// `#[async_trait]`, so async methods are boxed per call while sync
    /// defaults monomorphize and inline away.
    fn upstream_request_body_disposition(
        &self,
        _session: &Session,
        _ctx: &Self::CTX,
    ) -> UpstreamRequestBodyDisposition {
        UpstreamRequestBodyDisposition::Ordinary
    }
```

In `lib.rs`, change :94 to
`pub use proxy_trait::{FailToProxy, ProxyHttp, UpstreamRequestBodyDisposition};`
and add `UpstreamRequestBodyDisposition` to the `prelude` list (:96-98).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS. Do not commit yet.

---

### Task 7: H1 disposition application (commit 3, part 2)

**Files:**
- Modify: `pingora-proxy/src/proxy_h1.rs` (free function after the imports ending :22; call site in `proxy_to_h1` before `begin_request_body_replay` at :84; tests module at end of file)

**Interfaces:**
- Consumes: `UpstreamRequestBodyDisposition`, `upstream_request_body_disposition` (Task 6).
- Produces: `fn apply_upstream_body_disposition(request: &mut RequestHeader, disposition: UpstreamRequestBodyDisposition) -> Result<()>` (private to `proxy_h1.rs`).

- [ ] **Step 1: Write the failing unit tests**

Add at the end of `proxy_h1.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_framing() -> RequestHeader {
        let mut request = RequestHeader::build("POST", b"/", None).unwrap();
        request.insert_header(header::CONTENT_LENGTH, "12").unwrap();
        request
            .insert_header(header::TRANSFER_ENCODING, "gzip")
            .unwrap();
        request
    }

    #[test]
    fn streamed_disposition_uses_h1_chunked_framing() {
        let mut request = request_with_framing();
        apply_upstream_body_disposition(&mut request, UpstreamRequestBodyDisposition::Streamed)
            .unwrap();

        assert!(request.headers.get(header::CONTENT_LENGTH).is_none());
        assert_eq!(
            request.headers.get(header::TRANSFER_ENCODING).unwrap(),
            "chunked"
        );
    }

    #[test]
    fn bodyless_disposition_removes_request_framing() {
        let mut request = request_with_framing();
        apply_upstream_body_disposition(&mut request, UpstreamRequestBodyDisposition::Bodyless)
            .unwrap();

        assert!(request.headers.get(header::CONTENT_LENGTH).is_none());
        assert!(request.headers.get(header::TRANSFER_ENCODING).is_none());
    }

    #[test]
    fn ordinary_disposition_preserves_request_framing() {
        let mut request = request_with_framing();
        apply_upstream_body_disposition(&mut request, UpstreamRequestBodyDisposition::Ordinary)
            .unwrap();

        assert_eq!(request.headers.get(header::CONTENT_LENGTH).unwrap(), "12");
        assert_eq!(
            request.headers.get(header::TRANSFER_ENCODING).unwrap(),
            "gzip"
        );
    }
}
```

If `header` (the `http::header` module) is not already in scope, add
`use http::header;` to the imports.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-proxy --lib proxy_h1`
Expected: FAIL — `apply_upstream_body_disposition` not found.

- [ ] **Step 3: Implement**

After the imports (ends :22):

```rust
fn apply_upstream_body_disposition(
    request: &mut RequestHeader,
    disposition: UpstreamRequestBodyDisposition,
) -> Result<()> {
    match disposition {
        UpstreamRequestBodyDisposition::Ordinary => {}
        UpstreamRequestBodyDisposition::Bodyless => {
            request.remove_header(&header::CONTENT_LENGTH);
            request.remove_header(&header::TRANSFER_ENCODING);
        }
        UpstreamRequestBodyDisposition::Streamed => {
            // An H1 request with neither Content-Length nor
            // Transfer-Encoding is a zero-length body, so removal alone
            // would be a correctness bug.
            request.remove_header(&header::CONTENT_LENGTH);
            request.remove_header(&header::TRANSFER_ENCODING);
            request.insert_header(header::TRANSFER_ENCODING, "chunked")?;
        }
    }
    Ok(())
}
```

At the call site in `proxy_to_h1`, immediately before
`if let Err(e) = session.as_mut().begin_request_body_replay().await {` (:84):

```rust
        let body_disposition = self.inner.upstream_request_body_disposition(session, ctx);
        if let Err(e) = apply_upstream_body_disposition(&mut req, body_disposition) {
            return (false, true, Some(e));
        }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS. Do not commit yet.

---

### Task 8: H2 disposition application + e2e framing test + commit (commit 3, part 3)

**Files:**
- Modify: `pingora-proxy/src/proxy_h2.rs` (free functions after imports ending :24; call sites in `proxy_to_h2` around :121-160; tests at end of file)
- Modify: `pingora-proxy/tests/test_request_body_seam.rs` (append one test)

**Interfaces:**
- Consumes: `UpstreamRequestBodyDisposition` (Task 6), `SeamProxy`/`spawn_scripted_upstream` (Task 2).
- Produces: `fn apply_upstream_body_disposition(request: &mut RequestHeader, disposition: UpstreamRequestBodyDisposition)` and `fn upstream_headers_end_stream(disposition: UpstreamRequestBodyDisposition, send_end_stream: bool, body_empty: bool) -> bool` (private to `proxy_h2.rs`); `SeamProxy` honors the `x-disposition: streamed` request header.

- [ ] **Step 1: Write the failing unit tests**

Add at the end of `proxy_h2.rs` (next to the existing `test_update_authority`
free test at :987):

```rust
#[test]
fn test_streamed_disposition_removes_h2_framing_and_keeps_stream_open() {
    let mut request = RequestHeader::build("POST", b"/", None).unwrap();
    request.insert_header(CONTENT_LENGTH, "0").unwrap();
    request
        .insert_header(http::header::TRANSFER_ENCODING, "chunked")
        .unwrap();

    apply_upstream_body_disposition(&mut request, UpstreamRequestBodyDisposition::Streamed);

    assert!(request.headers.get(CONTENT_LENGTH).is_none());
    assert!(request
        .headers
        .get(http::header::TRANSFER_ENCODING)
        .is_none());
    assert!(!upstream_headers_end_stream(
        UpstreamRequestBodyDisposition::Streamed,
        true,
        true
    ));
}

#[test]
fn test_bodyless_disposition_ends_h2_on_headers() {
    assert!(upstream_headers_end_stream(
        UpstreamRequestBodyDisposition::Bodyless,
        false,
        false
    ));
}

#[test]
fn test_ordinary_h2_headers_eos_preserves_legacy_gate() {
    assert!(upstream_headers_end_stream(
        UpstreamRequestBodyDisposition::Ordinary,
        true,
        true
    ));
    assert!(!upstream_headers_end_stream(
        UpstreamRequestBodyDisposition::Ordinary,
        true,
        false
    ));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-proxy --lib proxy_h2`
Expected: FAIL — functions not found.

- [ ] **Step 3: Implement the H2 side**

After the imports (ends :24):

```rust
fn apply_upstream_body_disposition(
    request: &mut RequestHeader,
    disposition: UpstreamRequestBodyDisposition,
) {
    match disposition {
        UpstreamRequestBodyDisposition::Ordinary => {}
        UpstreamRequestBodyDisposition::Bodyless | UpstreamRequestBodyDisposition::Streamed => {
            request.remove_header(&http::header::CONTENT_LENGTH);
            request.remove_header(&http::header::TRANSFER_ENCODING);
        }
    }
}

fn upstream_headers_end_stream(
    disposition: UpstreamRequestBodyDisposition,
    send_end_stream: bool,
    body_empty: bool,
) -> bool {
    match disposition {
        UpstreamRequestBodyDisposition::Ordinary => send_end_stream && body_empty,
        UpstreamRequestBodyDisposition::Bodyless => true,
        UpstreamRequestBodyDisposition::Streamed => false,
    }
}
```

Call sites in `proxy_to_h2`:

1. Before `begin_request_body_replay` (:124):

```rust
        let body_disposition = self.inner.upstream_request_body_disposition(session, ctx);
        apply_upstream_body_disposition(&mut req, body_disposition);
```

2. Replace the END_STREAM decision at :152-154, currently:

```rust
        // send END_STREAM on HEADERS
        let send_header_eos = send_end_stream && body_empty;
        debug!("send END_STREAM on HEADERS: {send_end_stream}");
```

with:

```rust
        // send END_STREAM on HEADERS
        let send_header_eos =
            upstream_headers_end_stream(body_disposition, send_end_stream, body_empty);
        debug!("send END_STREAM on HEADERS: {send_header_eos}");
```

3. Gate the empty-DATA EOS path (:161), currently
`if !send_end_stream && body_empty {`, to:

```rust
        if body_disposition == UpstreamRequestBodyDisposition::Ordinary
            && !send_end_stream
            && body_empty
        {
```

- [ ] **Step 4: Wire the test hook into `SeamProxy` and add the e2e test**

In `pingora-proxy/tests/test_request_body_seam.rs`, add inside the
`impl ProxyHttp for SeamProxy` block:

```rust
    fn upstream_request_body_disposition(
        &self,
        session: &pingora_proxy::Session,
        _ctx: &Self::CTX,
    ) -> pingora_proxy::UpstreamRequestBodyDisposition {
        if session.req_header().headers.get("x-disposition").is_some() {
            pingora_proxy::UpstreamRequestBodyDisposition::Streamed
        } else {
            pingora_proxy::UpstreamRequestBodyDisposition::Ordinary
        }
    }
```

Add a raw-capture upstream helper and the test at the end of the file:

```rust
/// An upstream that captures everything it receives for one connection,
/// answers 200 once a complete chunked body (terminated by "0\r\n\r\n") or
/// Content-Length body arrives, then closes.
pub fn spawn_capturing_upstream() -> (u16, Arc<std::sync::Mutex<Vec<u8>>>) {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_ret = captured.clone();
    let listener = RT.block_on(async { TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let port = listener.local_addr().unwrap().port();

    RT.spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let mut guard = captured.lock().unwrap();
            guard.extend_from_slice(&buf[..n]);
            let done = guard.windows(5).any(|w| w == b"0\r\n\r\n");
            drop(guard);
            if done {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
                return;
            }
        }
    });
    (port, captured_ret)
}

#[test]
fn streamed_disposition_rewrites_h1_upstream_framing() {
    init();
    let (port, captured) = spawn_capturing_upstream();

    RT.block_on(async {
        let client = reqwest::Client::new();
        let res = client
            .post("http://127.0.0.1:6191/")
            .header("x-port", port.to_string())
            .header("x-disposition", "streamed")
            .body("hello world!") // reqwest sends Content-Length: 12
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    });

    let bytes = captured.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&bytes).to_lowercase();
    assert!(
        text.contains("transfer-encoding: chunked"),
        "upstream request must be chunked: {text}"
    );
    assert!(
        !text.contains("content-length"),
        "stale client content-length must be removed: {text}"
    );
    assert!(
        text.contains("hello world!"),
        "body bytes must arrive intact: {text}"
    );
}
```

- [ ] **Step 5: Run everything and commit**

Run: `cargo test -p pingora-proxy --lib && cargo test -p pingora-proxy --test test_request_body_seam && cargo test -p pingora-core --lib`
Expected: PASS.

```bash
cargo fmt --all
git -C /Users/caohao/ws2/pingora add pingora-proxy/src/proxy_trait.rs pingora-proxy/src/proxy_h1.rs pingora-proxy/src/proxy_h2.rs pingora-proxy/src/lib.rs pingora-proxy/tests/test_request_body_seam.rs
git -C /Users/caohao/ws2/pingora commit -m "Add upstream request body disposition for application-selected framing

Ordinary preserves current behavior. Bodyless removes request framing
headers (H2 sends END_STREAM on HEADERS). Streamed selects unknown-length
framing: explicit chunked for an H1 upstream, DATA END_STREAM for H2.
Each pump applies its own protocol rules so applications never need to
know the selected upstream protocol.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Terminate types and trait hooks (commit 4, part 1)

**Files:**
- Modify: `pingora-proxy/src/proxy_common.rs` (append)
- Modify: `pingora-proxy/src/proxy_trait.rs` (type + two async trait methods + test)
- Modify: `pingora-proxy/src/lib.rs` (re-exports and prelude)

**Interfaces:**
- Produces:
  - `pub(crate) enum DownstreamRequestOutcome { Complete(bool), Terminate }` in `proxy_common.rs` (crate-internal; Tasks 10-11 consume via `use crate::proxy_common::*`, which both pump files already do).
  - `pub enum RequestBodyAction { Continue, Terminate }` (public).
  - Trait methods `request_body_filter_action` (async, default delegates to `request_body_filter` and returns `Continue`) and `request_trailer_filter` (async, default `Continue`).

- [ ] **Step 1: Write the failing source-compatibility test**

Append to the `mod tests` in `proxy_trait.rs` (created in Task 6):

```rust
    struct LegacyBodyFilter;

    #[async_trait]
    impl ProxyHttp for LegacyBodyFilter {
        type CTX = bool;

        fn new_ctx(&self) -> Self::CTX {
            false
        }

        async fn upstream_peer(
            &self,
            _session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<Box<HttpPeer>> {
            unreachable!("upstream_peer is not used by this body-hook unit test")
        }

        async fn request_body_filter(
            &self,
            _session: &mut Session,
            _body: &mut Option<Bytes>,
            _end_of_stream: bool,
            ctx: &mut Self::CTX,
        ) -> Result<()> {
            *ctx = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn action_hook_defaults_to_legacy_body_filter_and_continue() {
        let io = tokio_test::io::Builder::new().build();
        let mut session = Session::new_h1(Box::new(io));
        let mut body = Some(Bytes::from_static(b"body"));
        let mut ctx = false;
        let app = LegacyBodyFilter;

        let action = app
            .request_body_filter_action(&mut session, &mut body, false, &mut ctx)
            .await
            .unwrap();

        assert!(ctx, "the legacy request_body_filter must have run");
        assert_eq!(action, RequestBodyAction::Continue);

        let trailer_action = app
            .request_trailer_filter(&mut session, &mut ctx)
            .await
            .unwrap();
        assert_eq!(trailer_action, RequestBodyAction::Continue);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-proxy --lib proxy_trait`
Expected: FAIL — `RequestBodyAction` / methods not found.

- [ ] **Step 3: Implement**

Append to `proxy_common.rs`:

```rust
/// The outcome of the downstream half of a proxy exchange.
///
/// This is the crate-internal half of the termination contract: an
/// application-selected terminal action travels as a typed outcome, never as
/// a generic `Error`, so it can bypass retry classification and
/// `fail_to_proxy` response generation. The future response-side streaming
/// hook reuses this same channel.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DownstreamRequestOutcome {
    /// Normal completion; the bool is downstream connection reusability.
    Complete(bool),
    /// Application termination: proxying of this request stops here. The
    /// application has already finished the downstream response.
    Terminate,
}
```

In `proxy_trait.rs`, next to `UpstreamRequestBodyDisposition`:

```rust
/// The action selected by an application request-body hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestBodyAction {
    /// Continue proxying the current request-body event.
    Continue,
    /// Stop proxying this request after the application completed or aborted
    /// the downstream response.
    Terminate,
}
```

Inside the trait, after `request_body_filter`:

```rust
    /// Handle one request-body event and decide whether proxying should
    /// continue.
    ///
    /// Before returning [`RequestBodyAction::Terminate`] the application must
    /// have finished the downstream response itself — either by writing a
    /// local reply, or by observing an already-committed response and
    /// abandoning. Pingora never writes a response on the terminate path.
    ///
    /// The default preserves source compatibility by invoking
    /// [`Self::request_body_filter`] and returning
    /// [`RequestBodyAction::Continue`].
    async fn request_body_filter_action(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<RequestBodyAction>
    where
        Self::CTX: Send + Sync,
    {
        self.request_body_filter(session, body, end_of_stream, ctx)
            .await?;
        Ok(RequestBodyAction::Continue)
    }

    /// Handle the presence of actual downstream request trailer fields.
    ///
    /// The hook runs after the trailer-presence fact is established and
    /// before a trailer-free synthetic request-body EOF could be delivered.
    /// Pingora does not expose or forward the trailer fields.
    async fn request_trailer_filter(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<RequestBodyAction>
    where
        Self::CTX: Send + Sync,
    {
        Ok(RequestBodyAction::Continue)
    }
```

In `lib.rs`, extend the re-export to
`pub use proxy_trait::{FailToProxy, ProxyHttp, RequestBodyAction, UpstreamRequestBodyDisposition};`
and add `RequestBodyAction` to the prelude.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS. Do not commit yet.

---

### Task 10: H1 terminate propagation (commit 4, part 2)

**Files:**
- Modify: `pingora-proxy/src/proxy_h1.rs` (`proxy_to_h1` join at :104-136; `proxy_handle_downstream` signature :280-284, early-buffer send :316-326, body branch :420-428, final return :611; `send_body_to_pipe` :774-821)

**Interfaces:**
- Consumes: `DownstreamRequestOutcome`, `RequestBodyAction`, trait hooks (Task 9); `Session::request_trailers_present` (Task 5).
- Produces: H1 pump semantics per design §3.2/§3.3. All hunks verbatim from the prototype patch except that `DownstreamRequestOutcome` now comes from `proxy_common` (do not redeclare it locally).

- [ ] **Step 1: Change `send_body_to_pipe` to return the outcome**

Change the signature (:774-780) return type from `Result<bool>` to
`Result<DownstreamRequestOutcome>`. Then, after the existing line
`let end_of_body = end_of_body || data.is_none();` (:784), insert the trailer
hook:

```rust
        if data.is_none()
            && session
                .downstream_session
                .request_trailers_present()
                .unwrap_or(false)
            && self.inner.request_trailer_filter(session, ctx).await?
                == RequestBodyAction::Terminate
        {
            return Ok(DownstreamRequestOutcome::Terminate);
        }
```

Replace the application hook call (:791-793), currently:

```rust
        self.inner
            .request_body_filter(session, &mut data, end_of_body, ctx)
            .await?;
```

with:

```rust
        if self
            .inner
            .request_body_filter_action(session, &mut data, end_of_body, ctx)
            .await?
            == RequestBodyAction::Terminate
        {
            return Ok(DownstreamRequestOutcome::Terminate);
        }
```

Wrap the two remaining returns: `:806` `return Ok(false);` becomes
`return Ok(DownstreamRequestOutcome::Complete(false));`, and the final
`Ok(end_of_body)` (:821) becomes
`Ok(DownstreamRequestOutcome::Complete(end_of_body))`.

- [ ] **Step 2: Propagate through `proxy_handle_downstream`**

Change its return type (:284) from `Result<bool>` to
`Result<DownstreamRequestOutcome>` and the final `Ok(reuse_downstream)`
(:611) to `Ok(DownstreamRequestOutcome::Complete(reuse_downstream))`.

Early-buffer send (:316-326): capture the outcome and bail on terminate:

```rust
            let outcome = self
                .send_body_to_pipe(
                    session,
                    buffer,
                    downstream_state.is_done(),
                    send_permit,
                    ctx,
                )
                .await?;
            if outcome == DownstreamRequestOutcome::Terminate {
                session.set_keepalive(None);
                return Ok(outcome);
            }
```

Body branch (:420-428): replace

```rust
                    let request_done = self.send_body_to_pipe(
                        session,
                        body,
                        is_body_done,
                        send_permit,
                        ctx,
                    )
                    .await?;
                    downstream_state.maybe_finished(request_done);
```

with:

```rust
                    let outcome = self.send_body_to_pipe(
                        session,
                        body,
                        is_body_done,
                        send_permit,
                        ctx,
                    )
                    .await?;
                    if outcome == DownstreamRequestOutcome::Terminate {
                        session.set_keepalive(None);
                        return Ok(outcome);
                    }
                    let DownstreamRequestOutcome::Complete(request_done) = outcome else {
                        unreachable!("terminal request-body outcome returned above");
                    };
                    downstream_state.maybe_finished(request_done);
```

- [ ] **Step 3: Replace the join in `proxy_to_h1`**

Replace :107-115, currently:

```rust
        // start bi-directional streaming
        let ret = tokio::try_join!(
            self.proxy_handle_downstream(
                session,
                tx_downstream,
                rx_upstream,
                ctx,
                &mut downstream_custom_message_writer
            ),
            self.proxy_handle_upstream(client_session, tx_upstream, rx_downstream),
        );
```

with:

```rust
        // start bi-directional streaming
        let ret = {
            let downstream = self.proxy_handle_downstream(
                session,
                tx_downstream,
                rx_upstream,
                ctx,
                &mut downstream_custom_message_writer,
            );
            let upstream = self.proxy_handle_upstream(client_session, tx_upstream, rx_downstream);
            tokio::pin!(downstream);
            tokio::pin!(upstream);

            tokio::select! {
                downstream_result = &mut downstream => {
                    match downstream_result {
                        Ok(DownstreamRequestOutcome::Terminate) => {
                            // Dropping the sibling future immediately stops both upstream
                            // response reads and request-body writes.
                            None
                        }
                        Ok(DownstreamRequestOutcome::Complete(reuse)) => {
                            Some(upstream.await.map(|upstream| (DownstreamRequestOutcome::Complete(reuse), upstream)))
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
                upstream_result = &mut upstream => {
                    Some(match upstream_result {
                        Ok(upstream) => downstream.await.map(|downstream| (downstream, upstream)),
                        Err(e) => Err(e),
                    })
                }
            }
        };
```

And the result match (:133-136), currently:

```rust
        match ret {
            Ok((downstream_can_reuse, _upstream)) => (downstream_can_reuse, true, None),
            Err(e) => (false, false, Some(e)),
        }
```

with:

```rust
        match ret {
            None => (false, false, None),
            Some(Ok((DownstreamRequestOutcome::Complete(downstream_can_reuse), _upstream))) => {
                (downstream_can_reuse, true, None)
            }
            Some(Ok((DownstreamRequestOutcome::Terminate, _upstream))) => (false, false, None),
            Some(Err(e)) => (false, false, Some(e)),
        }
```

- [ ] **Step 4: Run the lib suite**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS (no behavior change on the default path; `Terminate` is
unreachable until an application overrides the hooks). Do not commit yet.

---

### Task 11: H2 terminate propagation (commit 4, part 3)

**Files:**
- Modify: `pingora-proxy/src/proxy_h2.rs` (join at :186-221, result match :217-230, `bidirection_down_to_up` :270-345, body branch :384-395, final return :558, `send_body_to2` :720-764)

**Interfaces:**
- Consumes: same as Task 10.
- Produces: H2 pump semantics per design §3.2/§3.3 (terminate → `send_reset(CANCEL)` on the upstream stream, downstream connection stays reusable).

All hunks verbatim from the prototype patch (again minus the local enum).

- [ ] **Step 1: Change `send_body_to2`**

Signature (:727) `Result<bool>` → `Result<DownstreamRequestOutcome>`. At the
top of the function body insert the trailer hook (identical shape to H1):

```rust
        if data.is_none()
            && session
                .downstream_session
                .request_trailers_present()
                .unwrap_or(false)
            && self.inner.request_trailer_filter(session, ctx).await?
                == RequestBodyAction::Terminate
        {
            return Ok(DownstreamRequestOutcome::Terminate);
        }
```

Replace the `request_body_filter` call with the action-hook form (identical
to Task 10 Step 1). Wrap the remaining returns:
`return Ok(false);` → `return Ok(DownstreamRequestOutcome::Complete(false));`
and `Ok(end_of_body)` → `Ok(DownstreamRequestOutcome::Complete(end_of_body))`.

- [ ] **Step 2: Propagate through `bidirection_down_to_up`**

Return type (:344) → `Result<DownstreamRequestOutcome>`; final
`Ok(reuse_downstream)` (:558) →
`Ok(DownstreamRequestOutcome::Complete(reuse_downstream))`.

Retry-buffer send (:306-316): capture outcome, on terminate
`return Ok(outcome);` (no `set_keepalive` — H2 keeps the downstream
connection).

Body branch (:384-395): replace

```rust
                    match self.send_body_to2(session, body, is_body_done, client_body, ctx, write_timeout).await {
                        Ok(request_done) =>  {
                            downstream_state.maybe_finished(request_done);
                        },
```

with:

```rust
                    match self.send_body_to2(session, body, is_body_done, client_body, ctx, write_timeout).await {
                        Ok(DownstreamRequestOutcome::Complete(request_done)) =>  {
                            downstream_state.maybe_finished(request_done);
                        },
                        Ok(DownstreamRequestOutcome::Terminate) => {
                            return Ok(DownstreamRequestOutcome::Terminate);
                        },
```

(the existing `Err(e) => { ... }` arm stays unchanged).

- [ ] **Step 3: Replace the join in `proxy_to_h2`**

Replace the `tokio::try_join!` (:189-199) with the pinned `select!` block —
identical structure to Task 10 Step 3, with the H2 future names:
`self.bidirection_down_to_up(session, &mut client_body, rx, ctx, write_timeout, &mut downstream_custom_message_writer)`
as `downstream` and `pipe_up_to_down_response(client_session, tx)` as
`upstream`.

Result match (:217-230), currently starting
`Ok((downstream_can_reuse, _upstream)) => (downstream_can_reuse, None),`:

```rust
        match ret {
            None | Some(Ok((DownstreamRequestOutcome::Terminate, _))) => {
                client_body.send_reset(h2::Reason::CANCEL);
                (true, None)
            }
            Some(Ok((DownstreamRequestOutcome::Complete(downstream_can_reuse), _))) => {
                (downstream_can_reuse, None)
            }
            Some(Err(e)) => {
                // On application level upstream read timeouts, send RST_STREAM CANCEL,
                // we know we have not received END_STREAM at this point since we read timed out
                // TODO: implement for write timeouts?
                ...existing error-arm body unchanged...
            }
        }
```

(keep the existing `Err` arm body exactly as-is, only re-indented under
`Some(Err(e))`).

- [ ] **Step 4: Run the lib suite**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS. Do not commit yet.

---

### Task 12: Custom-pump action-hook wiring (commit 4, part 4)

**Files:**
- Modify: `pingora-proxy/src/proxy_custom.rs:687-694` (inside `send_body_to_custom`)

**Interfaces:**
- Consumes: `RequestBodyAction`, `request_body_filter_action` (Task 9).
- Produces: the defined fail-closed behavior of design §6.1 — the custom pump runs the action hook so filters execute uniformly, and `Terminate` is a typed error because the custom pump implements no terminate propagation.

- [ ] **Step 1: Implement**

In `send_body_to_custom`, replace :692-694, currently:

```rust
        self.inner
            .request_body_filter(session, &mut data, end_of_body, ctx)
            .await?;
```

with:

```rust
        if self
            .inner
            .request_body_filter_action(session, &mut data, end_of_body, ctx)
            .await?
            == RequestBodyAction::Terminate
        {
            // The custom pump has its own join structure and implements no
            // terminate propagation; fail closed instead of diverging
            // silently.
            return Error::e_explain(
                InternalError,
                "request-body terminate is not supported on custom connector sessions",
            );
        }
```

(`Error` and `InternalError` are already in scope in this file via the proxy
common imports; add `use` lines only if the compiler asks.)

- [ ] **Step 2: Run the lib suite**

Run: `cargo test -p pingora-proxy --lib`
Expected: PASS. Do not commit yet — commit 4 closes in Task 13.

---

### Task 13: Terminate integration tests + commit (commit 4, part 5)

**Files:**
- Modify: `pingora-proxy/tests/test_request_body_seam.rs` (extend `SeamProxy` and append tests)

**Interfaces:**
- Consumes: everything from Tasks 9-12; `spawn_scripted_upstream`, `UpstreamStep::Hang` (Task 2).
- Produces: `SeamProxy` honors `x-terminate-after-bytes: N` (write a 403 local reply mid-body, return `Terminate`) and `x-reject-trailers` (400 local reply from the trailer hook).

Note on the design's `request_future_is_send` compile-time assertion: this
test binary spawns the proxy services across threads via
`pingora_core::server::Server`, and `HttpServerApp` requires the request
future to be `Send` — so compiling and running this file IS the Send proof.
No separate assertion is written.

- [ ] **Step 1: Extend `SeamProxy` with the terminate hooks**

Add inside `impl ProxyHttp for SeamProxy`:

```rust
    async fn request_body_filter_action(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        _end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<pingora_proxy::RequestBodyAction> {
        let threshold: Option<usize> = session
            .req_header()
            .headers
            .get("x-terminate-after-bytes")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let Some(threshold) = threshold {
            ctx.body_bytes_seen += body.as_ref().map_or(0, |b| b.len());
            if ctx.body_bytes_seen >= threshold {
                let mut resp = ResponseHeader::build(403, None).unwrap();
                resp.insert_header("content-length", "6")?;
                session.write_response_header(Box::new(resp), false).await?;
                session
                    .write_response_body(Some(Bytes::from_static(b"denied")), true)
                    .await?;
                return Ok(pingora_proxy::RequestBodyAction::Terminate);
            }
        }
        Ok(pingora_proxy::RequestBodyAction::Continue)
    }

    async fn request_trailer_filter(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<pingora_proxy::RequestBodyAction> {
        if session.req_header().headers.get("x-reject-trailers").is_some() {
            let mut resp = ResponseHeader::build(400, None).unwrap();
            resp.insert_header("content-length", "0")?;
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(pingora_proxy::RequestBodyAction::Terminate);
        }
        Ok(pingora_proxy::RequestBodyAction::Continue)
    }
```

(`Session::write_response_header(Box<ResponseHeader>, bool)` and
`Session::write_response_body(Option<Bytes>, bool)` are the verified
signatures at `pingora-proxy/src/lib.rs:544` and `:569`.)

- [ ] **Step 2: Append the terminate tests**

```rust
/// A raw H1 client on one connection: send `first`, read until `expect` is
/// seen (with a 10s deadline), then optionally send `second` and report
/// whether the connection delivered more bytes or clean EOF.
async fn raw_h1_roundtrip(
    addr: &str,
    first: &[u8],
    expect: &[u8],
) -> (TcpStream, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(first).await.unwrap();
    let mut collected = Vec::new();
    let mut buf = vec![0u8; 16384];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !collected
        .windows(expect.len())
        .any(|w| w == expect)
    {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .expect("timed out waiting for expected response bytes")
            .unwrap();
        assert!(n > 0, "connection closed before expected bytes arrived");
        collected.extend_from_slice(&buf[..n]);
    }
    (stream, collected)
}

#[test]
fn h1_terminate_is_prompt_and_skips_generic_errors() {
    init();
    // The upstream reads the request and then hangs forever. Without
    // select!-driven sibling cancellation the request could not finish
    // until an upstream read timeout (none is configured here).
    let (port, counter) = spawn_scripted_upstream(vec![UpstreamStep::Hang]);

    let started = std::time::Instant::now();
    let (_stream, collected) = RT.block_on(async {
        let request = format!(
            "POST / HTTP/1.1\r\nHost: t\r\nx-port: {port}\r\n\
             x-terminate-after-bytes: 1\r\n\
             Transfer-Encoding: chunked\r\n\r\n\
             5\r\nhello\r\n"
        );
        raw_h1_roundtrip("127.0.0.1:6191", request.as_bytes(), b"denied").await
    });
    let elapsed = started.elapsed();

    let text = String::from_utf8_lossy(&collected);
    assert!(text.starts_with("HTTP/1.1 403"), "local reply expected: {text}");
    assert!(
        !text.contains("502") && !text.contains("500"),
        "terminate must not produce a generic proxy error response: {text}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "terminate must not wait for the hung upstream: {elapsed:?}"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1, "exactly one upstream attempt");
}

#[test]
fn h1_terminated_connection_is_not_reused() {
    init();
    let (port, _counter) = spawn_scripted_upstream(vec![UpstreamStep::Hang]);

    RT.block_on(async {
        let request = format!(
            "POST / HTTP/1.1\r\nHost: t\r\nx-port: {port}\r\n\
             x-terminate-after-bytes: 1\r\n\
             Transfer-Encoding: chunked\r\n\r\n\
             5\r\nhello\r\n"
        );
        let (mut stream, _collected) =
            raw_h1_roundtrip("127.0.0.1:6191", request.as_bytes(), b"denied").await;

        // The request body was never completed; the proxy must have marked
        // this connection non-reusable. A pipelined second request must not
        // be answered: expect clean EOF (or an error), never a response.
        let second = b"GET / HTTP/1.1\r\nHost: t\r\n\r\n";
        let _ = stream.write_all(second).await;
        let mut buf = vec![0u8; 4096];
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) => {}     // clean close: correct
            Ok(Err(_)) => {}    // reset: also acceptable
            Ok(Ok(n)) => panic!(
                "terminated H1 connection served a second request: {:?}",
                String::from_utf8_lossy(&buf[..n])
            ),
            Err(_) => panic!("connection neither closed nor errored within 5s"),
        }
    });
}

#[test]
fn h1_trailer_bearing_request_can_be_rejected() {
    init();
    let (port, counter) = spawn_scripted_upstream(vec![UpstreamStep::Hang]);

    let (_stream, collected) = RT.block_on(async {
        let request = format!(
            "POST / HTTP/1.1\r\nHost: t\r\nx-port: {port}\r\n\
             x-reject-trailers: 1\r\n\
             Transfer-Encoding: chunked\r\n\r\n\
             5\r\nhello\r\n0\r\nx-checksum: ok\r\n\r\n"
        );
        raw_h1_roundtrip("127.0.0.1:6191", request.as_bytes(), b"HTTP/1.1 400").await
    });

    let text = String::from_utf8_lossy(&collected);
    assert!(text.starts_with("HTTP/1.1 400"), "trailer rejection expected: {text}");
    // The trailer fact arrives with body EOF; the single hung upstream
    // attempt may or may not have received earlier body bytes, but no
    // SECOND attempt may exist.
    assert!(counter.load(Ordering::SeqCst) <= 1);
}

#[test]
fn h2_terminate_resets_stream_but_keeps_connection() {
    init();
    let (port, _counter) = spawn_scripted_upstream(vec![
        UpstreamStep::Hang,
        UpstreamStep::Respond(OK_KEEPALIVE),
    ]);

    RT.block_on(async {
        let tcp = TcpStream::connect("127.0.0.1:6192").await.unwrap();
        let (h2, connection) = h2::client::handshake(tcp).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut h2 = h2.ready().await.unwrap();

        // Stream A: terminated mid-body by the application.
        let req = http::Request::builder()
            .method("POST")
            .uri("http://t/")
            .header("x-port", port.to_string())
            .header("x-terminate-after-bytes", "1")
            .body(())
            .unwrap();
        let (response_a, mut body_a) = h2.send_request(req, false).unwrap();
        body_a.send_data(Bytes::from_static(b"hello"), false).unwrap();
        let resp_a = response_a.await.unwrap();
        assert_eq!(resp_a.status(), 403);

        // Stream B on the SAME connection must still work.
        let mut h2 = h2.ready().await.unwrap();
        let req = http::Request::builder()
            .method("GET")
            .uri("http://t/")
            .header("x-port", port.to_string())
            .body(())
            .unwrap();
        let (response_b, _) = h2.send_request(req, true).unwrap();
        let resp_b = response_b.await.unwrap();
        assert_eq!(
            resp_b.status(),
            200,
            "the downstream H2 connection must remain usable after a terminate"
        );
    });
}
```

- [ ] **Step 3: Run all suites**

Run: `cargo test -p pingora-proxy --lib && cargo test -p pingora-proxy --test test_request_body_seam && cargo test -p pingora-core --lib`
Expected: PASS. If `h2_terminate_resets_stream_but_keeps_connection` hangs on
stream A's response, verify Task 11 Step 3's `None` arm runs
`send_reset(CANCEL)` and that the local reply was written before `Terminate`
was returned.

- [ ] **Step 4: Format and commit**

```bash
cargo fmt --all
git -C /Users/caohao/ws2/pingora add pingora-proxy/src/proxy_common.rs pingora-proxy/src/proxy_trait.rs pingora-proxy/src/proxy_h1.rs pingora-proxy/src/proxy_h2.rs pingora-proxy/src/proxy_custom.rs pingora-proxy/src/lib.rs pingora-proxy/tests/test_request_body_seam.rs
git -C /Users/caohao/ws2/pingora commit -m "Add typed request-body terminate action with duplex cancellation

request_body_filter_action and request_trailer_filter let an application
finish or abort the downstream response mid-request-body. The terminate
outcome travels as a typed result through a select!-driven join: the
sibling upstream future is dropped immediately, no generic Error is
produced, H1 marks the downstream connection non-reusable, and H2 resets
only the current upstream stream. The custom-connector pump runs the
action hook and fails closed on Terminate.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 14: `request_retry_allowed` predicate + final verification (commit 5)

**Files:**
- Modify: `pingora-proxy/src/proxy_trait.rs` (one sync trait method + one test)
- Modify: `pingora-proxy/src/lib.rs` (`proxy_to_upstream` error paths :349-364; retry loop :878)
- Modify: `pingora-proxy/src/proxy_h1.rs:107` (`enable_retry_buffering` gate)
- Modify: `pingora-proxy/src/proxy_h2.rs:189` (`enable_retry_buffering` gate)
- Modify: `pingora-proxy/tests/test_request_body_seam.rs` (hook + test)

**Interfaces:**
- Consumes: `SeamProxy` scaffold; `final_response_committed` wiring context from Task 1.
- Produces: `fn request_retry_allowed(&self, _session: &Session, _ctx: &Self::CTX) -> bool` (sync, default `true`); `SeamProxy` honors the `x-no-retry` request header.

- [ ] **Step 1: Write the failing tests**

In `proxy_trait.rs` `mod tests`:

```rust
    #[test]
    fn retry_allowed_defaults_to_true() {
        let io = tokio_test::io::Builder::new().build();
        let session = Session::new_h1(Box::new(io));
        assert!(DefaultsOnly.request_retry_allowed(&session, &()));
    }
```

In `test_request_body_seam.rs`:

```rust
#[test]
fn retry_predicate_gates_reused_connection_retry() {
    init();
    // Upstream: request 1 succeeds (pools the connection); request 2 closes
    // without a response (retryable, pre-commit); request 3 would be the
    // retry attempt and succeeds.
    let run = |no_retry: bool| -> (usize, u16) {
        let (port, counter) = spawn_scripted_upstream(vec![
            UpstreamStep::Respond(OK_KEEPALIVE),
            UpstreamStep::CloseWithoutResponse,
            UpstreamStep::Respond(OK_KEEPALIVE),
        ]);
        let status = RT.block_on(async {
            let client = reqwest::Client::new();
            let res = client
                .get("http://127.0.0.1:6191/")
                .header("x-port", port.to_string())
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);

            let mut req = client
                .get("http://127.0.0.1:6191/")
                .header("x-port", port.to_string());
            if no_retry {
                req = req.header("x-no-retry", "1");
            }
            res_status(req.send().await)
        });
        (counter.load(Ordering::SeqCst), status)
    };

    // Control: retry allowed — the second client request is retried on a
    // fresh connection and succeeds; three upstream requests total.
    let (attempts, status) = run(false);
    assert_eq!(status, 200);
    assert_eq!(attempts, 3);

    // Predicate false: exactly one failed attempt, surfaced as 502.
    let (attempts, status) = run(true);
    assert_eq!(status, 502);
    assert_eq!(attempts, 2);
}

fn res_status(res: std::result::Result<reqwest::Response, reqwest::Error>) -> u16 {
    match res {
        Ok(r) => r.status().as_u16(),
        // A connection-level failure with no response counts as 0.
        Err(_) => 0,
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pingora-proxy --lib retry_allowed`
Expected: FAIL — method not found.

- [ ] **Step 3: Implement the trait method and the four consumption points**

Trait method in `proxy_trait.rs`, after `upstream_request_body_disposition`:

```rust
    /// Whether this request may make another upstream proxy attempt.
    ///
    /// Queried live at every retry decision point and never cached, so an
    /// application may flip from `true` to `false` mid-request. The
    /// application must keep the predicate monotonic (once `false`, never
    /// `true` again). Returning `false` also suppresses native request-body
    /// retry buffering.
    fn request_retry_allowed(&self, _session: &Session, _ctx: &Self::CTX) -> bool {
        true
    }
```

Consumption point 1 — retry-buffer gates. `proxy_h1.rs:107` and
`proxy_h2.rs:189`, currently `session.as_mut().enable_retry_buffering();` in
both files, become:

```rust
        if self.inner.request_retry_allowed(session, ctx) {
            session.as_mut().enable_retry_buffering();
        }
```

Consumption points 2 and 3 — in `lib.rs` `proxy_to_upstream` (:349-364).
Replace the success-arm error mapping:

```rust
                let retry_allowed = self.inner.request_retry_allowed(session, ctx);
                (
                    server_reused,
                    error.map(|e| {
                        let mut e =
                            self.inner
                                .error_while_proxy(&peer, session, e, ctx, client_reused);
                        if !retry_allowed {
                            e.retry = false.into();
                        }
                        e
                    }),
                )
```

and the connect-failure arm:

```rust
            Err(mut e) => {
                e.as_up();
                let mut new_err = self.inner.fail_to_connect(session, &peer, ctx, e);
                if !self.inner.request_retry_allowed(session, ctx) {
                    new_err.retry = false.into();
                }
                (false, Some(new_err.into_up()))
            }
```

Consumption point 4 — the retry loop (:878, as modified by Task 1):

```rust
                    let retry = error.retry() && self.inner.request_retry_allowed(&session, &ctx);
```

Wire the test hook into `SeamProxy`:

```rust
    fn request_retry_allowed(&self, session: &Session, _ctx: &Self::CTX) -> bool {
        session.req_header().headers.get("x-no-retry").is_none()
    }
```

- [ ] **Step 4: Run all suites**

Run: `cargo test -p pingora-proxy --lib && cargo test -p pingora-proxy --test test_request_body_seam && cargo test -p pingora-core --lib`
Expected: PASS.

- [ ] **Step 5: Full-repository verification**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
git -C /Users/caohao/ws2/pingora diff --check
```

Expected: fmt/diff clean; check green; clippy introduces no NEW warnings
relative to `git stash`-baseline (pre-existing repository warnings are
acceptable).

- [ ] **Step 6: Commit**

```bash
git -C /Users/caohao/ws2/pingora add pingora-proxy/src/proxy_trait.rs pingora-proxy/src/lib.rs pingora-proxy/src/proxy_h1.rs pingora-proxy/src/proxy_h2.rs pingora-proxy/tests/test_request_body_seam.rs
git -C /Users/caohao/ws2/pingora commit -m "Add request retry eligibility predicate

request_retry_allowed is consulted live at every retry decision point:
the retry-buffer gate, error_while_proxy wrapping, fail_to_connect, and
the retry loop. Returning false yields exactly one upstream attempt and
never populates the retry buffer.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
