//! Request-scoped Wasm state placeholder for builds without the `wasm` feature.
//!
//! No Wasm filter can be constructed, so no binding ever exists and the
//! request context's Wasm slot always stays empty.

use std::convert::Infallible;
use std::sync::Arc;

pub struct WasmRequestBinding {
    _never: Infallible,
}

#[derive(Default)]
pub(crate) struct WasmRequestState {
    _private: (),
}

impl WasmRequestState {
    pub(crate) fn install(&mut self, binding: Arc<WasmRequestBinding>) -> bool {
        drop(binding);
        false
    }

    pub(crate) fn get(&self, _identity: &Arc<()>) -> Option<Arc<WasmRequestBinding>> {
        None
    }

    pub(crate) fn finish(&mut self) -> bool {
        true
    }
}
