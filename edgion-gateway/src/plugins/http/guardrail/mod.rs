//! Bounded unary HTTP decisions for canonical semantic AI SSE windows.

mod attached;
mod plugin;
mod protocol;
mod response;

pub use attached::{AttachedGuardrail, GuardrailSummary};
pub use plugin::Guardrail;
