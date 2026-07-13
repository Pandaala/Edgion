//! Wasm plugin (proxy-wasm ABI host) — see plugin.rs for the filter impls,
//! vm_manager.rs for module caching / instance pooling, host_abi.rs for the
//! ABI bridge.

pub mod callout;
pub mod host_abi;
pub mod oci;
pub mod plugin;
pub mod shared_data;
pub mod vm_manager;

pub use plugin::WasmPlugin;
