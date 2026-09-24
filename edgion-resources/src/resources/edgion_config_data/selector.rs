//! `Selector` — a generic, hot-swappable profile selector.
//!
//! A `Selector` carries only the name of an *active profile* to choose from a set of
//! profiles the consumer already owns (git-authored, in the plugin base config). It
//! replaces the heavy `*Override` overlays for consumers whose only dynamic input is
//! "which preset profile is active".
//!
//! # Semantics (consumer-side, see the GlobalConnectionIpRestriction plugin)
//! - `active: Some(name)` — select profile `name`; if `name` is not one of the
//!   consumer's profiles, the consumer falls back to its base default (fail-to-base).
//! - `active: None` / absent — **no override**: the consumer keeps its base default.
//!   This is a deliberate no-op, never a bypass. A selector can only switch among the
//!   consumer's git-owned profiles; it can never disable the consumer.
//!
//! The envelope `enable`/`active` on `EdgionConfigDataSpec` still apply: `enable=false`
//! makes the consumer ignore this selector entirely (fail-to-base).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Generic active-profile selector carried by an `EdgionConfigData`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Selector {
    /// Name of the profile to activate. `None`/absent means "no override" — the
    /// consumer keeps its base default. Never a bypass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    /// Human-readable note. Purely informational (e.g. shown as a comment/label in a
    /// front-end); never consulted by the data plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/selector_test.rs"]
mod tests;
