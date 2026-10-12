//! Gateway-local EdgionDSL runtime facade.
//!
//! VM execution and builtin dispatch are owned by `edgion-gateway`.
//! Runtime-independent bytecode, errors, validation, and values are owned by
//! `edgion-dsl` and re-exported here for adjacent Gateway runtime modules.

pub(crate) mod body_ops;
pub(crate) mod body_scan_budget;
pub(crate) mod body_scanners;
pub mod builtins;
pub(crate) mod json_builtins;
pub mod vm;
pub(crate) mod xml_builtins;

pub use edgion_dsl::{bytecode, error, validator, value};
