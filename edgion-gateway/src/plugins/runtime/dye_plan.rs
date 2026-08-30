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
//! See `docs/superpowers/specs/2026-07-20-entry-level-dye-suite-design.md`.

use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::{CompiledTemplate, DyeOutcome, DyeRule, DyeSuite};

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
    fn compile(rule: &DyeRule) -> Result<Self, String> {
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
            conditions: rule.conditions.clone().map(CompiledPluginConditions::new),
        })
    }
}

/// Compiled entry-level dye plan, split by direction so the request-stage
/// anti-spoof strip only ever walks `request` and outcome recording only
/// ever walks `response`.
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
        let request = suite
            .request
            .iter()
            .map(CompiledDyeRule::compile)
            .collect::<Result<Vec<_>, _>>()?;
        let response = suite
            .response
            .iter()
            .map(CompiledDyeRule::compile)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self {
            request,
            response,
            type_prefix,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_plan_splits_directions_and_compiles_templates() {
        let suite: DyeSuite = serde_yaml::from_str(
            "request:\n  - name: X-T\nresponse:\n  - name: X-O\n    on: [fail-open]\n    value: 'v ${header:X-Id}'\n",
        )
        .unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert_eq!(plan.request.len(), 1);
        assert_eq!(plan.response.len(), 1);
        assert!(plan.response[0].value.is_some());
        assert_eq!(plan.type_prefix, "canary");
    }

    #[test]
    fn compile_plan_leaves_omitted_value_as_recorded_value() {
        let suite: DyeSuite = serde_yaml::from_str("response:\n  - name: X-O\n").unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert_eq!(plan.response.len(), 1);
        assert!(plan.response[0].value.is_none());
        assert_eq!(plan.response[0].on, vec![DyeOutcome::Success]);
    }

    #[test]
    fn compile_plan_carries_rule_conditions() {
        let suite: DyeSuite = serde_yaml::from_str(
            "response:\n  - name: X-O\n    conditions:\n      run:\n        allOf:\n          - type: keyExist\n            key:\n              type: header\n              name: X-Debug\n",
        )
        .unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert!(plan.response[0].conditions.is_some());
    }
}
