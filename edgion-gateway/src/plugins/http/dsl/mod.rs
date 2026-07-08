//! EdgionDSL — Custom plugin scripting language for Edgion gateway
//!
//! Provides a safe, sandboxed DSL for writing inline plugin logic.
//! Scripts are compiled to bytecode on the controller and executed
//! via a stack-based VM on the gateway.
//!
//! Architecture:
//!   Source → Parser(nom) → AST → Compiler → Bytecode   (edgion-dsl crate)
//!   Bytecode → VM(sandbox)                              (this module)
//!
//! The language core (parser/compiler/bytecode/validator) lives in the
//! standalone `edgion-dsl` crate so the controller runs the same
//! compile/validate path at resource ingest; `lang/` re-exports it and keeps
//! the VM + builtin dispatch here.
//!
//! Security model:
//!   Layer 0: Input limits (source size, parse budget, nesting depth)
//!   Layer 1: Compile-time validation (instruction count, locals, constants)
//!   Layer 2: Runtime fuel (step budget, loop budget, call budget)
//!   Layer 3: Fault isolation (catch_unwind + error policy)

pub mod config;
pub mod lang;
pub mod plugin;
pub mod runtime;

#[cfg(test)]
mod tests;
