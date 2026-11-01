//! Stable substrings emitted by this plugin's access log.
//!
//! These exist so the source-grep test in this module and the assertions in
//! `edgion-tests` reference the same literal. Changing a value here is a
//! deliberate cross-cutting change — `every_token_appears_in_plugin_source`
//! will fail until the plugin source and these consts agree.
//!
//! Not a public API. Only this crate (its tests) and `edgion-tests` consume it.
//!
//! Convention reference: skills/03-coding/observability/00-access-log.md
//! Spec: docs/superpowers/specs/2026-06-16-plugin-log-tokens-design.md

pub const HASH_KEY: &str = "hash key=";
pub const HASH_WB: &str = "hash_wb→";
pub const HASH_PASS: &str = "hash_pass=";
pub const CONF_MATCH_KEY: &str = "conf_match key=";
pub const CONF_MATCH_NO_ENTRY: &str = "no_entry, fallthrough";
pub const DIRECT: &str = "direct=";
pub const DIRECT_UNKNOWN: &str = "unknown, fallthrough";
// Trailing space distinguishes this from `push_err("failover_self_ref")`
// and similar error-suffix substrings.
pub const FAILOVER_REDIRECT: &str = "failover ";
pub const LOCAL_REGION_PASS: &str = "local_region, pass";
pub const SETTING_DISABLED: &str = "setting_disabled";
pub const SETTING_MISSING: &str = "setting_missing";
pub const SETTING_TYPE_MISMATCH: &str = "setting_type_mismatch";
pub const SETTING_INVALID: &str = "setting_invalid";

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/region_route/log_consts_test.rs"]
mod tests;
