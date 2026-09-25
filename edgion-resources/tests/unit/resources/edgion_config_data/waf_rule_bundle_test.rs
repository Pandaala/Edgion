use super::{WafBundleMember, WafRuleBundleData};

fn bundle() -> WafRuleBundleData {
    let mut bundle = WafRuleBundleData {
        version: "2026.1".into(),
        profile: "strict".into(),
        provenance: "example/revision-1".into(),
        digest: None,
        roots: vec!["rules/main.conf".into()],
        rules: vec![WafBundleMember {
            name: "rules/main.conf".into(),
            content: "SecRule REQUEST_URI \"@contains admin\" \"id:1001,phase:1,deny\"".into(),
        }],
        phrase_assets: vec![WafBundleMember {
            name: "assets/words.txt".into(),
            content: "password\nsecret".into(),
        }],
    };
    bundle.digest = Some(bundle.compute_digest());
    bundle
}

#[test]
fn bundle_integrity_and_paths_are_strict() {
    let mut value = bundle();
    assert!(value.validate().is_ok());

    value.phrase_assets[0].content.push_str(" changed");
    assert!(value.validate().is_err());
    value.digest = Some(value.compute_digest());
    assert!(value.validate().is_ok());

    value.rules[0].name = "../outside.conf".into();
    value.roots[0] = value.rules[0].name.clone();
    value.digest = Some(value.compute_digest());
    assert!(value.validate().is_err());
}

#[test]
fn bundle_without_a_digest_uses_structural_validation() {
    let mut value = bundle();
    value.digest = None;
    assert!(value.validate().is_ok());
    assert!(serde_json::to_value(&value).unwrap().get("digest").is_none());
    value.rules[0].content.push_str("# locally edited");
    assert!(value.validate().is_ok());
    value.digest = Some(String::new());
    assert!(value.validate().is_err());
}

#[test]
fn bundle_rejects_duplicate_names_and_missing_roots() {
    let mut value = bundle();
    value.phrase_assets[0].name = value.rules[0].name.clone();
    value.digest = Some(value.compute_digest());
    assert!(value.validate().is_err());

    let mut value = bundle();
    value.roots[0] = "rules/missing.conf".into();
    value.digest = Some(value.compute_digest());
    assert!(value.validate().is_err());
}

#[test]
fn member_content_is_redacted_in_admin_and_debug_but_present_on_wire() {
    let value = bundle();
    let wire = serde_json::to_string(&value).unwrap();
    assert!(wire.contains("password"));

    let _guard = crate::redact::AdminRedactGuard::new();
    let admin = serde_json::to_string(&value).unwrap();
    assert!(!admin.contains("password"));
    assert!(!admin.contains("SecRule"));
    assert!(admin.contains("[redacted]"));
    assert!(!format!("{value:?}").contains("password"));
}

#[test]
fn member_declaration_order_does_not_change_digest() {
    let mut value = bundle();
    value.rules.push(WafBundleMember {
        name: "rules/helper.conf".into(),
        content: "# helper".into(),
    });
    let digest = value.compute_digest();
    value.rules.reverse();
    assert_eq!(digest, value.compute_digest());
}
