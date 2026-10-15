use super::*;
#[test]
fn policy_ancestor_admission_is_explicit_and_bounded() {
    assert!(!policy_ancestor_permitted(None, Some("team/gateway")));
    assert!(PreparedPolicyAncestorAdmission::Unrestricted.permits(None));
    let scoped = PreparedPolicyAncestorAdmission::Scoped(vec!["team/gateway".into()]).validated();
    assert!(scoped.permits(Some("team/gateway")));
    assert!(!scoped.permits(Some("other/gateway")));
    assert!(!scoped.permits(None));
    for keys in [vec!["bad/key/extra".into()], vec!["team/gateway".into(); 17]] {
        assert!(!PreparedPolicyAncestorAdmission::Scoped(keys)
            .validated()
            .permits(Some("team/gateway")));
    }
    let wire = serde_json::to_string(&scoped).unwrap();
    assert_eq!(
        serde_json::from_str::<PreparedPolicyAncestorAdmission>(&wire).unwrap(),
        scoped
    );
}
