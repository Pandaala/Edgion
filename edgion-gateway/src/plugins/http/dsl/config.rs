//! DSL plugin configuration — re-exported from `edgion_resources`.
//!
//! `DslConfig` and `DslErrorPolicy` are defined in
//! `edgion_resources::resources::edgion_plugins::plugin_configs::dsl`
//! so the schema layer carries no engine dependency.
//! The compiled DSL VM (`DslPlugin`) remains in this gateway module.

pub use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::DslConfig;
