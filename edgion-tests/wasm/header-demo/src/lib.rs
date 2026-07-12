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
//! - `x-wasm-read-body: 1`  -> read the shared `bodybytes` counter into request
//!   header `x-wasm-bodybytes` (proves the response-body hook fired).
//! - otherwise -> add request header `x-wasm-added: yes`.
//!
//! Response phase (`on_http_response_headers`): add `x-wasm-resp: yes`.
//!
//! Response body (`on_http_response_body`, only when wired at the
//! upstreamResponseBodyFilter stage): read-only; adds each chunk's byte count to
//! the shared `bodybytes` counter.
//!
//! Background: `on_configure` sets a 1s tick; `on_tick` increments the shared
//! `ticks` counter.

use std::time::Duration;

use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, ContextType, LogLevel, Status};

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> { Box::new(DemoRoot) });
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

struct DemoRoot;

impl Context for DemoRoot {}

impl RootContext for DemoRoot {
    fn on_configure(&mut self, _plugin_configuration_size: usize) -> bool {
        // Drive a 1s background tick that bumps the shared `ticks` counter.
        self.set_tick_period(Duration::from_millis(1000));
        true
    }

    fn on_tick(&mut self) {
        bump_shared_counter(self, "ticks", 1);
    }

    fn create_http_context(&self, _context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(HeaderDemo::default()))
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
}

impl Context for HeaderDemo {
    fn on_http_call_response(&mut self, _token: u32, num_headers: usize, _body_size: usize, _num_trailers: usize) {
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
        }
        Action::Continue
    }
}
