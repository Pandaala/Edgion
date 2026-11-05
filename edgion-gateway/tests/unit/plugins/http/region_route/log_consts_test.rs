use super::*;

const PLUGIN_SRC: &str = include_str!("../../../../../src/plugins/http/region_route/plugin.rs");

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
        ("SETTING_DISABLED", SETTING_DISABLED),
        ("SETTING_MISSING", SETTING_MISSING),
        ("SETTING_TYPE_MISMATCH", SETTING_TYPE_MISMATCH),
        ("SETTING_INVALID", SETTING_INVALID),
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
