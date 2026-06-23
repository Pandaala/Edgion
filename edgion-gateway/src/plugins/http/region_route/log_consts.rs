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
pub const OVERLAY_DISABLED: &str = "overlay_disabled";
pub const OVERLAY_MISSING: &str = "overlay_missing";
pub const OVERLAY_TYPE_MISMATCH: &str = "overlay_type_mismatch";
pub const OVERLAY_INVALID: &str = "overlay_invalid";

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN_SRC: &str = include_str!("plugin.rs");

    #[test]
    fn every_token_appears_in_plugin_source() {
        let tokens: &[(&str, &str)] = &[
            ("HASH_KEY", HASH_KEY),
            ("HASH_WB", HASH_WB),
            ("HASH_PASS", HASH_PASS),
            ("CONF_MATCH_KEY", CONF_MATCH_KEY),
            ("CONF_MATCH_NO_ENTRY", CONF_MATCH_NO_ENTRY),
            ("DIRECT", DIRECT),
            ("DIRECT_UNKNOWN", DIRECT_UNKNOWN),
            ("FAILOVER_REDIRECT", FAILOVER_REDIRECT),
            ("LOCAL_REGION_PASS", LOCAL_REGION_PASS),
            ("OVERLAY_DISABLED", OVERLAY_DISABLED),
            ("OVERLAY_MISSING", OVERLAY_MISSING),
            ("OVERLAY_TYPE_MISMATCH", OVERLAY_TYPE_MISMATCH),
            ("OVERLAY_INVALID", OVERLAY_INVALID),
        ];
        for (name, value) in tokens {
            assert!(
                PLUGIN_SRC.contains(value),
                "token {} (`{}`) not present in region_route/plugin.rs — \
                 the plugin source changed without updating log_consts.rs, \
                 or vice versa",
                name,
                value,
            );
        }
    }
}
