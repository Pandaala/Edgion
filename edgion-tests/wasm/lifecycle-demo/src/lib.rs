//! Real-SDK fixture: assertions depend on HTTP/root object fields, not merely
//! shared counters. Shared data exposes the result to the host test harness.
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, ContextType};
use std::time::Duration;

proxy_wasm::main! {{
    proxy_wasm::set_root_context(|_| Box::new(Root::default()));
}}

#[derive(Default)]
struct Root {
    ticks: u64,
}
impl Context for Root {}
impl RootContext for Root {
    fn on_configure(&mut self, _: usize) -> bool {
        self.set_tick_period(Duration::from_millis(100));
        true
    }
    fn on_tick(&mut self) {
        self.ticks += 1;
        self.set_shared_data("root-ticks", Some(&self.ticks.to_le_bytes()), None)
            .unwrap();
    }
    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
    fn create_http_context(&self, _: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(Request::default()))
    }
}

#[derive(Default)]
struct Request {
    marker: String,
    request_done: bool,
    response_done: bool,
    bytes: u64,
    chunks: u64,
}
impl Request {
    fn publish(&self, suffix: &str, value: u64) {
        self.set_shared_data(&format!("{}:{suffix}", self.marker), Some(&value.to_le_bytes()), None)
            .unwrap();
    }
}
impl Context for Request {
    fn on_done(&mut self) -> bool {
        let key = format!("{}:done", self.marker);
        let (old, _) = self.get_shared_data(&key);
        let count = old.map(|v| u64::from_le_bytes(v.try_into().unwrap())).unwrap_or(0);
        self.publish("done", count + 1);
        true
    }
}
impl HttpContext for Request {
    fn on_http_request_headers(&mut self, _: usize, eos: bool) -> Action {
        assert!(!eos, "the selected whole-body hook follows headers");
        self.marker = self.get_http_request_header("x-lifecycle-id").unwrap();
        if self.marker == "deny" {
            self.send_http_response(403, vec![], None);
        }
        Action::Continue
    }
    fn on_http_request_body(&mut self, size: usize, eos: bool) -> Action {
        assert!(!self.marker.is_empty());
        assert!(!self.request_done);
        assert!(eos);
        assert_eq!(self.get_http_request_body(0, size).unwrap(), self.marker.as_bytes());
        self.request_done = true;
        self.set_http_request_header("x-context-request", Some(&self.marker));
        Action::Continue
    }
    fn on_http_response_headers(&mut self, _: usize, eos: bool) -> Action {
        assert!(self.request_done);
        self.set_http_response_header("x-context-response", Some(&self.marker));
        self.response_done = eos;
        if eos {
            self.publish("eos", 1);
        }
        Action::Continue
    }
    fn on_http_response_body(&mut self, size: usize, eos: bool) -> Action {
        assert!(self.request_done);
        assert!(!self.response_done, "no callback after EOS");
        assert_eq!(self.get_http_response_body(0, size).unwrap_or_default().len(), size);
        self.chunks += 1;
        self.bytes += size as u64;
        self.publish("chunks", self.chunks);
        self.publish("bytes", self.bytes);
        self.response_done = eos;
        if eos {
            self.publish("eos", 1);
        }
        Action::Continue
    }
    fn on_http_response_trailers(&mut self, _: usize) -> Action {
        assert!(!self.response_done, "body must not end before real trailers");
        assert_eq!(
            self.get_http_response_trailer("x-lifecycle-trailer").as_deref(),
            Some(self.marker.as_str())
        );
        self.response_done = true;
        self.publish("trailers", 1);
        self.publish("eos", 1);
        Action::Continue
    }
}
