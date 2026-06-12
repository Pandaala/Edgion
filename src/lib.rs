//! Umbrella package — integration-test examples and helper binaries only.
//!
//! All Edgion code now lives in the workspace crates: `edgion-resources`,
//! `edgion-common`, `edgion-gateway`, `edgion-controller`, and `edgion-cli`.
//! This package retains no library code of its own; it exists solely to host
//! the `examples/` targets and the `cache_residue` helper binary, which depend
//! on those crates. Binaries set their own `#[global_allocator]`; this library
//! deliberately sets none.
