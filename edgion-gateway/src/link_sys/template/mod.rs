//! Shared template evaluation for LinkSys consumers.
//!
//! Hosts the compiled-template evaluation logic used by both the `ctx_set`
//! HTTP plugin and the webhook provider. The template structure is fixed at
//! config-load time, so user-supplied values that happen to contain `${…}`
//! are emitted verbatim -- they are never re-parsed as template directives.

pub mod engine;

pub use engine::evaluate_template;
