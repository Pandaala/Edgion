//! Compiled entry-level dye plan.
//!
//! `PluginEntry::dye` (schema-level `DyeSuite`) is compiled once, at
//! `PluginRuntime` build time, into a `CompiledDyePlan` carried by the entry's
//! conditional wrapper. Validation (`DyeSuite::validation_error`) already ran
//! on ingest and guarantees every template parses, every name is a valid
//! non-denied header token, and no Dye value template references `secretRef`
//! (conditions retain their normal reference support) — so a
//! `CompiledTemplate::compile` failure here is a bug, not an operator error.
//! It is still surfaced through the normal compile-error vec (fail-closed,
//! matching every other `PluginRuntime` compile error), never a panic.
//!
//! See `skills/01-architecture/02-gateway/entry-level-dye.md`.

use std::sync::Arc;

use edgion_resources::resources::common::request_template::CompiledTemplate;
use edgion_resources::resources::edgion_plugins::{DyeOutcome, DyeRule, DyeSuite};

use super::conditions::CompiledPluginConditions;

/// One compiled dye rule: which outcomes render (`on`), the pre-compiled
/// operator value template (`None` => render the plugin-recorded outcome
/// value), and an optional render gate (never gates the anti-spoof strip).
pub struct CompiledDyeRule {
    pub name: String,
    pub on: Vec<DyeOutcome>,
    /// Compiled operator value template; None => recorded value.
    pub value: Option<CompiledTemplate>,
    pub(crate) conditions: Option<CompiledPluginConditions>,
}

impl CompiledDyeRule {
    fn compile(
        rule: &DyeRule,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Result<Self, String> {
        let value = match &rule.value {
            Some(v) => Some(
                CompiledTemplate::compile(v)
                    .map_err(|e| format!("dye rule '{}': value template error: {}", rule.name, e))?,
            ),
            None => None,
        };
        Ok(Self {
            name: rule.name.clone(),
            on: rule.on.clone(),
            value,
            conditions: rule
                .conditions
                .as_ref()
                .map(|conditions| CompiledPluginConditions::with_ip(conditions, ip)),
        })
    }
}

/// Compiled entry-level dye plan, split by direction so the request-stage
/// anti-spoof strip only walks `request`; outcome recording renders each
/// direction through the canonical session adapter.
pub struct CompiledDyePlan {
    /// Rules rendered onto the upstream request (success outcomes only).
    pub request: Vec<CompiledDyeRule>,
    /// Rules rendered onto the downstream response.
    pub response: Vec<CompiledDyeRule>,
    /// Stable snake_case value prefix for this entry's plugin type
    /// (`EdgionPlugin::dye_type_prefix`). Validation only permits a dye
    /// suite on a participating (non-empty-prefix) plugin type, so this is
    /// never the empty string in a plan that actually reaches the loops.
    pub type_prefix: &'static str,
}

impl CompiledDyePlan {
    /// Compile an entry's suite. Validation already ran; a template that
    /// fails to compile here is a bug — surface via the compile error vec.
    pub fn compile(suite: &DyeSuite, type_prefix: &'static str) -> Result<Arc<Self>, String> {
        Self::with_ip(suite, type_prefix, None)
    }
    pub(crate) fn with_ip(
        suite: &DyeSuite,
        type_prefix: &'static str,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Result<Arc<Self>, String> {
        let request = suite
            .request
            .iter()
            .map(|rule| CompiledDyeRule::compile(rule, ip))
            .collect::<Result<Vec<_>, _>>()?;
        let response = suite
            .response
            .iter()
            .map(|rule| CompiledDyeRule::compile(rule, ip))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self {
            request,
            response,
            type_prefix,
        }))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/dye_plan_test.rs"]
mod tests;
