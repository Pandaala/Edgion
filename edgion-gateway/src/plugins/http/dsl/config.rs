//! DSL plugin configuration — re-exported from `edgion_resources`.
//!
//! `edgion_resources` owns the shared `DslConfig` schema and its static
//! compile/validation wrapper. The runtime-independent parser, compiler, and
//! validator live in `edgion_dsl`. `DslPlugin`, VM execution, builtin dispatch,
//! and runtime integration remain in `edgion_gateway`.

pub use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::DslConfig;
