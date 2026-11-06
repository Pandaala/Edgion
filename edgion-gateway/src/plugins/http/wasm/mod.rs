//! Wasm plugin (proxy-wasm ABI host) — see plugin.rs for the filter impls,
//! vm_manager.rs for plugin-owned runtimes / instance pooling, host_abi.rs for the
//! ABI bridge.
//!
//! Without the `wasm` cargo feature only the Wasmtime-free parts stay compiled
//! (admission limits and their configuration); `plugin` and `request` become
//! placeholders so every Wasm plugin entry fails closed.

pub mod admission;
mod limits;

#[cfg(feature = "wasm")]
mod attached;
#[cfg(feature = "wasm")]
mod callbacks;
#[cfg(feature = "wasm")]
pub mod callout;
#[cfg(feature = "wasm")]
pub mod host_abi;
#[cfg(feature = "wasm")]
pub mod oci;
#[cfg(feature = "wasm")]
pub mod plugin;
#[cfg(feature = "wasm")]
pub mod request;
#[cfg(feature = "wasm")]
pub mod shared_data;
#[cfg(feature = "wasm")]
pub mod vm_manager;

#[cfg(not(feature = "wasm"))]
#[path = "plugin_disabled.rs"]
pub mod plugin;
#[cfg(not(feature = "wasm"))]
#[path = "request_disabled.rs"]
pub mod request;

pub use plugin::WasmPlugin;
