//! Wasm plugin placeholder for builds without the `wasm` feature.
//!
//! TLS validation always fails, so the plugin runtime records every Wasm entry
//! as invalid and the route fails closed. No filter is ever constructed.

use std::convert::Infallible;

use edgion_common::link_sys_core::outbound_tls::{GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection};
use edgion_resources::resources::refs::plugin_fields::BoundPluginEntry;

use crate::plugins::runtime::{RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter};

const WASM_NOT_COMPILED: &str = "Wasm is not compiled into this gateway build (cargo feature `wasm`)";

pub struct WasmPlugin {
    _never: Infallible,
}

impl WasmPlugin {
    pub(crate) fn create_attached(
        _entry: &BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<(Box<dyn RequestFilter>, Box<dyn UpstreamResponse>)> {
        None
    }

    pub(crate) fn validate_tls_entry<'a>(
        _entry: &'a BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Option<ValidatedLocalTlsSelection<'a>>, String> {
        Err(WASM_NOT_COMPILED.to_string())
    }

    pub fn create_request(
        _entry: &BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<Box<dyn RequestFilter>> {
        None
    }

    pub fn create_response_filter(
        _entry: &BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<Box<dyn UpstreamResponseFilter>> {
        None
    }

    pub fn create_body_filter(
        _entry: &BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<Box<dyn UpstreamResponseBodyFilter>> {
        None
    }
}
