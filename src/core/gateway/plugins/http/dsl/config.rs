//! DSL plugin configuration — re-exported from the `types` schema layer.
//!
//! The serializable `DslConfig` / `DslErrorPolicy` now live in
//! `types::resources::edgion_plugins::plugin_configs::dsl` so the schema layer
//! carries no engine dependency. The compiled DSL VM (`DslPlugin`) stays in this
//! gateway module. Existing `super::config::DslConfig` references keep working
//! via this re-export.

pub use crate::types::resources::edgion_plugins::plugin_configs::dsl::{DslConfig, DslErrorPolicy};
