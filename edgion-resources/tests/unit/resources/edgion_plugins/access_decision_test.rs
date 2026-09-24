use super::*;

#[test]
fn allow_by_group_carries_name() {
    let d = AccessDecision::AllowByGroup(Arc::from("office"));
    match d {
        AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "office"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn deny_by_whitelist_miss_no_payload() {
    let d = AccessDecision::DenyByWhitelistMiss;
    assert!(matches!(d, AccessDecision::DenyByWhitelistMiss));
}

#[test]
fn default_carries_action() {
    let d = AccessDecision::Default(DefaultAction::Deny);
    assert!(matches!(d, AccessDecision::Default(DefaultAction::Deny)));
}

#[test]
fn clone_works() {
    let d = AccessDecision::DenyByGroup(Arc::from("blocklist"));
    let c = d.clone();
    match (d, c) {
        (AccessDecision::DenyByGroup(a), AccessDecision::DenyByGroup(b)) => {
            assert_eq!(a, b);
            assert!(Arc::ptr_eq(&a, &b));
        }
        _ => panic!("clone changed variant"),
    }
}
