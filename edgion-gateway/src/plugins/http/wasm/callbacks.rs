//! Typed exports resolved once per instance; callback dispatch never performs
//! repeated export-name lookup or signature checking.
use super::host_abi::HostState;
use wasmtime::{Instance, Store, TypedFunc};

type HttpHook = TypedFunc<(i32, i32, i32), i32>;
type CalloutHook = TypedFunc<(i32, i32, i32, i32, i32), ()>;

pub(super) struct GuestCallbacks {
    request_headers: Option<HttpHook>,
    request_body: Option<HttpHook>,
    response_headers: Option<HttpHook>,
    response_body: Option<HttpHook>,
    pub response_trailers: Option<TypedFunc<(i32, i32), i32>>,
    pub create: Option<TypedFunc<(i32, i32), ()>>,
    pub done: Option<TypedFunc<i32, i32>>,
    pub delete: Option<TypedFunc<i32, ()>>,
    pub tick: Option<TypedFunc<i32, ()>>,
    pub callout: Option<CalloutHook>,
}

impl GuestCallbacks {
    pub fn resolve(instance: Instance, store: &mut Store<HostState>) -> Self {
        Self {
            request_headers: instance.get_typed_func(&mut *store, "proxy_on_request_headers").ok(),
            request_body: instance.get_typed_func(&mut *store, "proxy_on_request_body").ok(),
            response_headers: instance.get_typed_func(&mut *store, "proxy_on_response_headers").ok(),
            response_body: instance.get_typed_func(&mut *store, "proxy_on_response_body").ok(),
            response_trailers: instance.get_typed_func(&mut *store, "proxy_on_response_trailers").ok(),
            create: instance.get_typed_func(&mut *store, "proxy_on_context_create").ok(),
            done: instance.get_typed_func(&mut *store, "proxy_on_done").ok(),
            delete: instance.get_typed_func(&mut *store, "proxy_on_delete").ok(),
            tick: instance.get_typed_func(&mut *store, "proxy_on_tick").ok(),
            callout: instance.get_typed_func(&mut *store, "proxy_on_http_call_response").ok(),
        }
    }

    pub fn http(&self, hook: &str) -> Option<HttpHook> {
        match hook {
            "proxy_on_request_headers" => self.request_headers.clone(),
            "proxy_on_request_body" => self.request_body.clone(),
            "proxy_on_response_headers" => self.response_headers.clone(),
            "proxy_on_response_body" => self.response_body.clone(),
            _ => None,
        }
    }
}
