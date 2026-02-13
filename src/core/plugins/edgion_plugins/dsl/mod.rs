//! EdgionDSL — Custom plugin scripting language for Edgion gateway
//!
//! Provides a safe, sandboxed DSL for writing inline plugin logic.
//! Scripts are compiled to bytecode on the controller and executed
//! via a stack-based VM on the gateway.
//!
//! Architecture:
//!   Source → Parser(nom) → AST → Compiler → Bytecode → VM(sandbox)
//!
//! Security model:
//!   Layer 0: Input limits (source size, parse budget, nesting depth)
//!   Layer 1: Compile-time validation (instruction count, locals, constants)
//!   Layer 2: Runtime fuel (step budget, loop budget, call budget)
//!   Layer 3: Fault isolation (catch_unwind + error policy)

pub mod lang;
pub mod config;
pub mod plugin;
