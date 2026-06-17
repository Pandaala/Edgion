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
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_active_some() {
        let s = Selector {
            active: Some("lockdown".to_string()),
            description: Some("switch to lockdown".to_string()),
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["active"], "lockdown");
        assert_eq!(j["description"], "switch to lockdown");
        let back: Selector = serde_json::from_value(j).unwrap();
        assert_eq!(back.active.as_deref(), Some("lockdown"));
    }

    #[test]
    fn active_none_omitted_in_output() {
        let s = Selector {
            active: None,
            description: None,
        };
        let j = serde_json::to_value(&s).unwrap();
        // skip_serializing_if drops both fields → empty object.
        assert!(j.get("active").is_none());
        assert!(j.get("description").is_none());
    }

    #[test]
    fn deserialize_empty_object_is_no_op_selector() {
        let s: Selector = serde_json::from_str("{}").unwrap();
        assert!(s.active.is_none());
        assert!(s.description.is_none());
    }
}
