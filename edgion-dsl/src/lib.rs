//! EdgionDSL language core — parser, compiler, bytecode, static validation.
//!
//! Extracted from `edgion-gateway` so the control plane (via
//! `edgion-resources`) can run the same compile/validate path the data plane
//! uses, surfacing DSL compile errors on the K8s status condition at ingest
//! instead of only at gateway plugin construction. Runtime execution (the VM
//! and builtin dispatch) stays in `edgion-gateway` — this crate is
//! engine-free and has no workspace-internal dependencies.
//!
//! Architecture:
//!   Source → Parser(nom) → AST → Compiler → Bytecode  (this crate)
//!   Bytecode → VM(sandbox)                             (edgion-gateway)

pub mod ast;
pub mod bytecode;
pub mod compiler;
pub mod error;
pub mod parser;
mod script;
pub mod validator;
pub mod value;

pub use script::{check_redis_link_names, resolve_script};
