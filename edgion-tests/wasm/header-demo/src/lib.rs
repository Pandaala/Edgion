//! Reference proxy-wasm guest for Edgion integration + host tests.
//!
//! Built with the official proxy-wasm Rust SDK (v0.2) so the suites exercise a
//! real SDK-produced module through the hand-written host ABI, not just a WAT
//! fixture. One guest covers every capability the host implements; behavior is
//! selected by request headers so unrelated scenarios never interfere.
//!
//! ## Contract
//!
//! Request phase (`on_http_request_headers`), checked in order:
//! - `x-wasm-panic: 1`     -> panic (host recovery test).
//! - `x-wasm-deny: 1`      -> local 403 with body `wasm denied`.
//! - `x-wasm-callout-url: <origin>` -> dispatch a GET `<origin>/verify` callout
//!   forwarding the request `Authorization` header, then Pause. On the callback:
//!   callout status 200 -> resume (adds `x-wasm-added: yes`); any other status
//!   or a failed callout (0 headers) -> local 403 body `callout denied`.
//!   `<origin>` must be listed in the plugin's `calloutAllowlist`.
//! - `x-wasm-read-ticks: 1` -> read the shared `ticks` counter into request
//!   header `x-wasm-ticks` (proves on_tick liveness + shared-data read).
//! - `x-wasm-read-body: 1` -> read the shared response `bodybytes` counter into
//!   request header `x-wasm-bodybytes`.
//! - `x-wasm-read-request-body: 1` -> read the shared `requestbodybytes`
//!   counter, callback count, and terminal count into request headers
//!   `x-wasm-requestbodybytes`, `x-wasm-requestbodycalls`, and
//!   `x-wasm-requestbodyeos`.
//! - `x-wasm-read-callout: 1` -> read the shared `calloutticks` counter into
//!   request header `x-wasm-calloutticks` (proves the tick-phase callout loop:
//!   on_tick dispatch -> root-context callback -> shared-data write).
//! - `x-wasm-rewrite-body: 1` -> the request-body hook exercises replace,
//!   insert, append, and delete against a body of at least six bytes.
//! - `x-wasm-append-body: 1` -> append `created` to the request body, including
//!   a bodyless request.
//! - otherwise -> add request header `x-wasm-added: yes`.
//!
//! Response phase (`on_http_response_headers`): add `x-wasm-resp: yes`.
//!
//! Response body (`on_http_response_body`, only when wired at the
//! upstreamResponseBodyFilter stage): read-only; adds each chunk's byte count to
//! the shared `bodybytes` counter. With `pluginConfig: response-body-probe`, a
//! nonempty chunk also returns Pause so the host's unsupported-Pause metric
//! can prove this separate plugin instance ran the hook.
//!
//! Background: `on_configure` sets a 1s tick; `on_tick` increments the shared
//! `ticks` counter. When `pluginConfig` is a callout origin (e.g.
//! `http://127.0.0.1:30040`), every tick also dispatches a GET `<origin>/verify`
//! callout with `authorization: Bearer valid-token`; a 200 response bumps the
//! shared `calloutticks` counter (classic "poll a control endpoint from on_tick,
//! publish via shared data" pattern). The origin must be in `calloutAllowlist`;
//! a rejected dispatch is ignored so the same artifact still serves scenarios
//! without callout config.

use std::time::Duration;

use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, ContextType, LogLevel, Status};

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> { Box::new(DemoRoot::default()) });
}}

// ---- Shared-data u64 counter helpers (CAS retry loop) ----

fn read_u64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(buf)
}

/// Add `delta` to the shared counter at `key` with a CAS retry loop.
fn bump_shared_counter<C: Context + ?Sized>(ctx: &C, key: &str, delta: u64) {
    for _ in 0..16 {
        let (val, cas) = ctx.get_shared_data(key);
        let cur = val.as_deref().map(read_u64).unwrap_or(0);
        let next = cur.wrapping_add(delta).to_le_bytes();
        match ctx.set_shared_data(key, Some(&next), cas) {
            Ok(()) => return,
            Err(Status::CasMismatch) => continue,
            Err(_) => return,
        }
    }
}

fn read_shared_counter<C: Context + ?Sized>(ctx: &C, key: &str) -> u64 {
    let (val, _) = ctx.get_shared_data(key);
    val.as_deref().map(read_u64).unwrap_or(0)
}

// ---- Root context: tick + http-context factory ----

#[derive(Default)]
struct DemoRoot {
    /// Callout origin polled from `on_tick`, taken from `pluginConfig`.
    /// None (no/blank config) disables the tick-callout branch.
    callout_origin: Option<String>,
    response_body_probe: bool,
}

impl Context for DemoRoot {
    /// Tick-callout callback (root context): a 200 from `<origin>/verify`
    /// bumps the shared `calloutticks` counter; failures are ignored.
    fn on_http_call_response(&mut self, _token: u32, num_headers: usize, _body_size: usize, _num_trailers: usize) {
        if num_headers > 0 && self.get_http_call_response_header(":status").as_deref() == Some("200") {
            bump_shared_counter(self, "calloutticks", 1);
        }
    }
}

impl RootContext for DemoRoot {
    fn on_configure(&mut self, _plugin_configuration_size: usize) -> bool {
        // pluginConfig, when present, is the origin to poll from on_tick.
        let config = self
            .get_plugin_configuration()
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .filter(|s| !s.is_empty());
        self.response_body_probe = config.as_deref() == Some("response-body-probe");
        self.callout_origin = config.filter(|_| !self.response_body_probe);
        // Drive a 1s background tick that bumps the shared `ticks` counter.
        self.set_tick_period(Duration::from_millis(1000));
        true
    }

    fn on_tick(&mut self) {
        bump_shared_counter(self, "ticks", 1);
        // Tick-phase callout: poll the configured origin. A rejected
        // dispatch (unarmed or non-allowlisted) is ignored so the same
        // artifact serves scenarios without callout config.
        if let Some(origin) = self.callout_origin.clone() {
            let _ = self.dispatch_http_call(
                &origin,
                vec![
                    (":method", "GET"),
                    (":path", "/verify"),
                    (":authority", "auth-callout"),
                    ("authorization", "Bearer valid-token"),
                ],
                None,
                vec![],
                Duration::from_secs(2),
            );
        }
    }

    fn create_http_context(&self, _context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(HeaderDemo {
            response_body_probe: self.response_body_probe,
            ..HeaderDemo::default()
        }))
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

// ---- Per-request HTTP context ----

#[derive(Default)]
struct HeaderDemo {
    /// Set while a callout is in flight so the callback knows this request is
    /// mid-auth (defensive; the host routes the callback by context anyway).
    awaiting_callout: bool,
    response_body_probe: bool,
}

impl Context for HeaderDemo {
    fn on_http_call_response(&mut self, _token: u32, num_headers: usize, _body_size: usize, _num_trailers: usize) {
        assert!(self.awaiting_callout, "callout must return to the originating HTTP object");
        self.awaiting_callout = false;
        // A failed callout arrives with 0 headers (proxy-wasm convention).
        if num_headers == 0 {
            self.send_http_response(403, vec![("x-wasm-callout", "failed")], Some(b"callout denied"));
            return;
        }
        match self.get_http_call_response_header(":status").as_deref() {
            Some("200") => {
                self.set_http_request_header("x-wasm-added", Some("yes"));
                self.resume_http_request();
            }
            _ => {
                self.send_http_response(403, vec![("x-wasm-callout", "denied")], Some(b"callout denied"));
            }
        }
    }
}

impl HttpContext for HeaderDemo {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        if self.get_http_request_header("x-wasm-panic").as_deref() == Some("1") {
            panic!("panic requested by x-wasm-panic header");
        }
        if self.get_http_request_header("x-wasm-deny").as_deref() == Some("1") {
            self.send_http_response(403, vec![("x-wasm-denied", "1")], Some(b"wasm denied"));
            return Action::Pause;
        }
        if self.get_http_request_header("x-wasm-read-ticks").as_deref() == Some("1") {
            let n = read_shared_counter(self, "ticks");
            self.set_http_request_header("x-wasm-ticks", Some(&n.to_string()));
        }
        if self.get_http_request_header("x-wasm-read-body").as_deref() == Some("1") {
            let n = read_shared_counter(self, "bodybytes");
            self.set_http_request_header("x-wasm-bodybytes", Some(&n.to_string()));
        }
        if self.get_http_request_header("x-wasm-read-request-body").as_deref() == Some("1") {
            for (counter, header) in [
                ("requestbodybytes", "x-wasm-requestbodybytes"),
                ("requestbodycalls", "x-wasm-requestbodycalls"),
                ("requestbodyeos", "x-wasm-requestbodyeos"),
            ] {
                let n = read_shared_counter(self, counter);
                self.set_http_request_header(header, Some(&n.to_string()));
            }
        }
        if self.get_http_request_header("x-wasm-read-callout").as_deref() == Some("1") {
            let n = read_shared_counter(self, "calloutticks");
            self.set_http_request_header("x-wasm-calloutticks", Some(&n.to_string()));
        }
        if let Some(origin) = self.get_http_request_header("x-wasm-callout-url") {
            let auth = self.get_http_request_header("authorization").unwrap_or_default();
            let headers = vec![
                (":method", "GET"),
                (":path", "/verify"),
                (":authority", "auth-callout"),
                ("authorization", auth.as_str()),
            ];
            match self.dispatch_http_call(&origin, headers, None, vec![], Duration::from_secs(2)) {
                Ok(_) => {
                    self.awaiting_callout = true;
                    return Action::Pause;
                }
                Err(_) => {
                    self.send_http_response(
                        503,
                        vec![("x-wasm-callout", "dispatch-error")],
                        Some(b"callout dispatch failed"),
                    );
                    return Action::Pause;
                }
            }
        }
        self.set_http_request_header("x-wasm-added", Some("yes"));
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_response_header("x-wasm-resp", Some("yes"));
        Action::Continue
    }

    fn on_http_response_body(&mut self, body_size: usize, _end_of_stream: bool) -> Action {
        // Read-only observation: count the chunk's bytes into shared state.
        if body_size > 0 {
            bump_shared_counter(self, "bodybytes", body_size as u64);
            if self.response_body_probe {
                return Action::Pause;
            }
        }
        Action::Continue
    }

    fn on_http_request_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        if self.get_http_request_header("x-wasm-rewrite-body").as_deref() == Some("1") && body_size >= 6 {
            self.set_http_request_body(1, 2, b"XYZ");
            self.set_http_request_body(1, 0, b"+");
            if let Some(body) = self.get_http_request_body(0, usize::MAX) {
                self.set_http_request_body(body.len(), 0, b"!");
            }
            self.set_http_request_body(2, 3, b"");
        } else if self.get_http_request_header("x-wasm-append-body").as_deref() == Some("1") {
            self.set_http_request_body(body_size, 0, b"created");
        }
        // Headers and body phases use distinct guest contexts, so identify
        // counter-read probes from the headers copied into this body phase.
        if self.get_http_request_header("x-wasm-read-request-body").as_deref() != Some("1") {
            bump_shared_counter(self, "requestbodybytes", body_size as u64);
            bump_shared_counter(self, "requestbodycalls", 1);
            if end_of_stream {
                bump_shared_counter(self, "requestbodyeos", 1);
            }
        }
        Action::Continue
    }
}
