//! Startup-only AI conversion safety policy. One immutable publication owns limits and admission.
use crate::ai::conversion::chat_to_anthropic::{budget::UnaryBudget, limits::UnaryLimits};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub conversion: ConversionConfig,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConversionConfig {
    pub chat_to_anthropic: ChatToAnthropicConfig,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatToAnthropicConfig {
    pub unary: UnaryConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnaryConfig {
    pub max_input_size: String,
    pub max_output_size: String,
    pub hold_timeout: String,
    pub memory_budget: String,
    pub max_json_depth: usize,
    pub max_json_nodes: usize,
    pub max_key_size: String,
    pub max_source_events: usize,
    pub max_metadata_size: String,
}
impl Default for UnaryConfig {
    fn default() -> Self {
        Self {
            max_input_size: "1MiB".into(),
            max_output_size: "1MiB".into(),
            hold_timeout: "120s".into(),
            memory_budget: "256MiB".into(),
            max_json_depth: 64,
            max_json_nodes: 16384,
            max_key_size: "256B".into(),
            max_source_events: 4096,
            max_metadata_size: "64KiB".into(),
        }
    }
}
pub(crate) struct AiRuntime {
    pub(crate) limits: UnaryLimits,
    pub(crate) budget: UnaryBudget,
}
static RUNTIME: OnceLock<AiRuntime> = OnceLock::new();
pub(crate) fn initialize(config: &AiConfig) -> Result<(), String> {
    let limits = UnaryLimits::resolve(&config.conversion.chat_to_anthropic.unary)?;
    let budget = UnaryBudget::new(limits.slots);
    debug_assert!(limits.reservation > 0, "resolved unary reservation is positive");
    RUNTIME
        .set(AiRuntime { limits, budget })
        .map_err(|_| "AI runtime already initialized; initialize once before serving".into())
}
pub(crate) fn runtime() -> Option<&'static AiRuntime> {
    RUNTIME.get()
}
/// Initialize the same startup runtime for an isolated transport integration target.
#[cfg(feature = "test-util")]
pub fn initialize_for_test(config: &AiConfig) -> Result<(), String> {
    initialize(config)
}
#[cfg(test)]
#[path = "../../tests/unit/ai/config_test.rs"]
mod tests;
