//! Gateway-local EdgionDSL runtime facade.
//!
//! VM execution and builtin dispatch are owned by `edgion-gateway`.
//! Runtime-independent bytecode, errors, validation, and values are owned by
//! `edgion-dsl` and re-exported here for adjacent Gateway runtime modules.

pub mod builtins;
pub mod vm;

pub use edgion_dsl::{bytecode, error, validator, value};
