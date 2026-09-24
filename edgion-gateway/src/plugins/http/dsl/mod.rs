//! EdgionDSL — Custom plugin scripting language for Edgion gateway
//!
//! Provides a safe, sandboxed DSL for writing inline plugin logic.
//! Source or bytecode is resolved and statically validated through the shared
//! `edgion-resources`/`edgion-dsl` path during control-plane resource ingest
//! and again when the gateway constructs the plugin. The gateway executes the
//! resulting `CompiledScript` through its stack-based VM and builtin dispatch.
//!
//! Architecture:
//!   Source → Parser(nom) → AST → Compiler → Bytecode   (edgion-dsl crate)
//!   Bytecode → VM(sandbox)                              (this module)
//!
//! The language core (parser/compiler/bytecode/validator) lives in the
//! standalone `edgion-dsl` crate. `lang/` exposes its runtime-adjacent
//! bytecode, error, validator, and value types alongside the gateway-owned VM
//! and builtin dispatch.
//!
//! Security model:
//!   Layer 0: Input limits (source size, parse budget, nesting depth)
//!   Layer 1: Shared compile-time validation (instruction count, locals, constants)
//!   Layer 2: Gateway post-deserialization validation and bounded regex compilation
//!   Layer 3: Runtime budgets (fuel, stack, collections, body scans, bounded outputs,
//!            including regex-replacement construction)
//!   Layer 4: Fault isolation (catch_unwind + error policy)

pub mod config;
pub mod lang;
pub mod plugin;
pub mod runtime;

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/dsl/mod_test.rs"]
mod tests;
