//! EdgionDSL language runtime — VM execution and builtin dispatch.
//!
//! The language core (parser, compiler, bytecode, static validator) lives in
//! the `edgion-dsl` crate so the control plane can run the same
//! compile/validate path. The re-exports below keep the historical
//! `dsl::lang::*` paths working for the runtime side that stays here.

mod body_scanners;
pub mod builtins;
pub mod vm;

pub use edgion_dsl::{ast, bytecode, compiler, error, parser, validator, value};
