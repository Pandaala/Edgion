//! Reference proxy-wasm guest for Edgion integration tests.
//!
//! Contract (asserted by edgion-tests suites):
//! - request headers: `x-wasm-panic: 1` -> panic (host recovery test);
//!   `x-wasm-deny: 1` -> local 403 response with body "wasm denied";
//!   otherwise -> add request header `x-wasm-added: yes`.
//! - response headers: add response header `x-wasm-resp: yes`.

use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, LogLevel};

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_http_context(|_context_id, _root_context_id| -> Box<dyn HttpContext> {
        Box::new(HeaderDemo)
    });
}}

struct HeaderDemo;

impl Context for HeaderDemo {}

impl HttpContext for HeaderDemo {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        if self.get_http_request_header("x-wasm-panic").as_deref() == Some("1") {
            panic!("panic requested by x-wasm-panic header");
        }
        if self.get_http_request_header("x-wasm-deny").as_deref() == Some("1") {
            self.send_http_response(403, vec![("x-wasm-denied", "1")], Some(b"wasm denied"));
            return Action::Pause;
        }
        self.set_http_request_header("x-wasm-added", Some("yes"));
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_response_header("x-wasm-resp", Some("yes"));
        Action::Continue
    }
}
