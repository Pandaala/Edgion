use super::*;

#[test]
fn stream_layer_metric_labels_are_stable() {
    assert_eq!(StreamLayer::Stage1.as_metric_label(), "stage1");
    assert_eq!(StreamLayer::Tcp.as_metric_label(), "tcp");
}

#[test]
fn fixed_decisions_preserve_callbacks_without_reason_allocations() {
    for (decision, expected) in [
        (AccessDecision::AllowByGroup("allow-group".into()), None),
        (AccessDecision::Default(DefaultAction::Allow), None),
        (AccessDecision::DenyByWhitelistMiss, Some("ip:deny:not-in-allow")),
        (AccessDecision::Default(DefaultAction::Deny), Some("ip:deny:default")),
    ] {
        let mut denied = 0;
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            access_decision_to_stream_result(decision, || denied += 1)
        });
        assert_eq!(stats.calls, 0);
        assert_eq!(denied, usize::from(expected.is_some()));
        match (result, expected) {
            (StreamPluginResult::Allow, None) => {}
            (StreamPluginResult::Deny(Cow::Borrowed(reason)), Some(expected)) => {
                assert_eq!(reason, expected);
            }
            result => panic!("unexpected decision mapping: {result:?}"),
        }
    }
}

#[test]
fn group_denial_preserves_owned_reason_and_single_callback() {
    let mut denied = 0;
    let result = access_decision_to_stream_result(AccessDecision::DenyByGroup("restricted".into()), || denied += 1);
    assert_eq!(denied, 1);
    let StreamPluginResult::Deny(Cow::Owned(reason)) = result else {
        panic!("dynamic denial must own its reason");
    };
    assert_eq!(reason, "ip:deny:group=restricted");
}
