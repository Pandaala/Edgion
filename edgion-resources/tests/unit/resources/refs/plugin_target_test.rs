use super::*;
use std::collections::HashMap;

#[test]
fn finite_default_candidates_preserve_raw_owner_and_reject_ambiguity() {
    for raw in ["plugins", "default/plugins"] {
        let mut sources = HashMap::from([(raw, 1)]);
        assert_eq!(
            classify("default/plugins", |key| sources.get(key).copied()).unwrap(),
            PluginTarget::Unique {
                raw_key: raw.to_owned(),
                source: 1
            }
        );
        sources.insert(if raw == "plugins" { "default/plugins" } else { "plugins" }, 1);
        assert_eq!(
            classify("default/plugins", |key| sources.get(key).copied()).unwrap(),
            PluginTarget::Ambiguous
        );
        assert_eq!(
            classify("other/plugins", |key| sources.get(key).copied()).unwrap(),
            PluginTarget::Missing
        );
    }
    assert_eq!(semantic_key("plugins").unwrap(), "default/plugins");
    assert_eq!(semantic_key("default/plugins").unwrap(), "default/plugins");
    assert_eq!(semantic_key("other/plugins").unwrap(), "other/plugins");
}

#[test]
fn invalid_semantic_identity_never_reads_and_empty_raw_scope_never_aliases() {
    for key in [
        "",
        "plugins",
        "/plugins",
        "default/",
        "default/x/y",
        "bad_ns/plugins",
        "default/Bad",
    ] {
        assert!(classify::<()>(key, |_| panic!("invalid identity must never read")).is_err());
    }
    for raw in ["", "/plugins", "default/", "default/x/y"] {
        assert!(semantic_key(raw).is_err());
    }
    assert_eq!(
        classify("default/plugins", |key| (key == "/plugins").then_some(())).unwrap(),
        PluginTarget::Missing
    );
}
