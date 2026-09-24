//! Envoy-compatible external processing (ext_proc) protocol bindings.
//!
//! Generated tonic client/types for a trimmed, wire-compatible subset of
//! `envoy.service.ext_proc.v3`. The gateway `ext_proc` plugin uses these to
//! drive an external processing service over a bidirectional gRPC stream.
pub mod proto;

#[cfg(test)]
#[path = "../../tests/unit/ext_proc/mod_test.rs"]
mod tests;
