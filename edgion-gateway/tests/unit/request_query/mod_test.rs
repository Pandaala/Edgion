use super::*;

fn query(raw: Option<&str>) -> RequestQuery {
    let mut owner = RequestQuery::default();
    owner.capture(raw.map(str::to_owned)).unwrap();
    owner
}

#[test]
fn capture_is_once_and_early_read_does_not_poison_entry() {
    let mut owner = RequestQuery::default();
    assert_eq!(owner.entry_param("a"), None);
    assert_eq!(owner.current_raw(), None);
    assert_eq!(owner.remove_current_names(&["a"]), Err(QueryError::NotCaptured));
    owner.capture(Some("a=entry".into())).unwrap();
    assert_eq!(owner.entry_param("a"), Some("entry"));
    assert_eq!(owner.capture(Some("a=second".into())), Err(QueryError::AlreadyCaptured));
    assert_eq!(owner.entry_param("a"), Some("entry"));
}

#[test]
fn auth_removal_preserves_exact_survivors_and_no_match_syntax() {
    for (raw, name, expected, changed) in [
        (None, "key", None, false),
        (Some(""), "key", Some(""), false),
        (Some("&&a=1&&b=2&"), "key", Some("&&a=1&&b=2&"), false),
        (Some("&&a=1&key=x&&b=2&"), "key", Some("a=1&b=2"), true),
        (
            Some("api%5Fkey=x&x=a%26b&api_key=again&&flag&x=+&"),
            "api_key",
            Some("x=a%26b&flag&x=+"),
            true,
        ),
        (Some("key&key=two"), "key", None, true),
        (Some("=secret&&keep=1"), "", Some("keep=1"), true),
        (Some("api%5Fkey=x"), "api%5Fkey", Some("api%5Fkey=x"), false),
        (Some("api+key=x&keep=1"), "api key", Some("keep=1"), true),
        (Some("bad%+x=x&keep=1"), "bad%+x", Some("keep=1"), true),
    ] {
        let mut owner = query(raw);
        assert_eq!(owner.remove_current_names(&[name]).unwrap(), changed, "{raw:?}");
        assert_eq!(owner.current_raw(), expected, "{raw:?}");
        assert_eq!(owner.entry_raw(), raw);
        assert_eq!(owner.projection_is_dirty(), changed);
    }
}

#[test]
fn consecutive_deletions_and_named_reads_do_not_force_render() {
    let mut owner = query(Some("a=1&token=x&b=2&secret=y&c=%2f"));
    assert!(owner.remove_current_names(&["token"]).unwrap());
    assert_eq!(owner.current_param("c"), Some("/"));
    assert!(owner.remove_current_names(&["secret"]).unwrap());
    let (_, selection) = owner.backing().unwrap();
    let selection = selection.unwrap();
    let expected = "a=1&b=2&c=%2f";
    assert!(selection.equals_raw(&owner.entry, Some(expected)));
    let (first, first_stats) = crate::test_allocation_counter::count_allocation_stats(|| owner.current_raw().unwrap());
    assert_eq!(first, expected);
    assert_eq!(first_stats.calls, 1);
    let (_, repeated_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert_eq!(owner.current_raw().unwrap().as_ptr(), first.as_ptr());
    });
    assert_eq!(repeated_stats.calls, 0);
    assert_eq!(owner.entry_param("token"), Some("x"));
}

#[test]
fn decoded_name_reuse_survives_auth_then_lb_and_more_deletions() {
    let mut owner = query(Some("token=x&encoded%5Fname=raw%2f&secret=y"));
    owner.remove_current_names(&["token"]).unwrap();
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert_eq!(owner.current_hash_arg_raw("encoded_name"), Some("raw%2f"));
        assert_eq!(owner.current_hash_arg_raw("encoded_name"), Some("raw%2f"));
    });
    assert_eq!(stats.calls, 0);
    owner.remove_current_names(&["secret"]).unwrap();
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert_eq!(owner.current_hash_arg_raw("encoded_name"), Some("raw%2f"));
    });
    assert_eq!(stats.calls, 0);
}

#[test]
fn replacement_preserves_entry_and_distinguishes_missing_and_empty() {
    let mut owner = query(Some("a=entry"));
    assert!(owner.replace_raw(Some("a=current".into())).unwrap());
    assert_eq!(owner.entry_param("a"), Some("entry"));
    assert_eq!(owner.current_param("a"), Some("current"));
    assert!(owner.replace_raw(Some(String::new())).unwrap());
    assert_eq!(owner.current_raw(), Some(""));
    assert!(owner.replace_raw(None).unwrap());
    assert_eq!(owner.current_raw(), None);
    assert!(!owner.replace_raw(None).unwrap());
}

#[test]
fn equal_replacement_does_not_render_or_change_revision() {
    let mut owner = query(Some("token=x&a=1"));
    owner.remove_current_names(&["token"]).unwrap();
    let raw = Some("a=1".to_owned());
    let revision = owner.revision;
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(!owner.replace_raw(raw).unwrap());
    });
    assert_eq!(stats.calls, 0);
    assert_eq!(owner.revision, revision);
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| owner.current_raw());
    assert_eq!(stats.calls, 1, "the equality check must not have rendered");
}

#[test]
fn snapshot_restore_reuses_render_and_decode_with_fresh_revision() {
    let mut owner = query(Some("token=x&a=entry"));
    owner.replace_raw(Some("token=y&a=baseline%20value".into())).unwrap();
    owner.remove_current_names(&["token"]).unwrap();
    let decoded = owner.current_param("a").unwrap().as_ptr();
    let rendered = owner.current_raw().unwrap().as_ptr();
    owner.capture_baseline().unwrap();
    owner.mark_synced();
    owner.replace_raw(Some("a=provider".into())).unwrap();
    let before_restore = owner.revision;
    owner.restore_baseline().unwrap();
    assert!(owner.revision > before_restore);
    assert!(owner.projection_is_dirty());
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert_eq!(owner.current_param("a").unwrap().as_ptr(), decoded);
        assert_eq!(owner.current_raw().unwrap().as_ptr(), rendered);
    });
    assert_eq!(stats.calls, 0);
    assert_eq!(owner.entry_param("a"), Some("entry"));
}

#[test]
fn revision_overflow_rejects_changes_without_partial_publication() {
    let mut owner = query(Some("token=x&a=1"));
    owner.revision = u64::MAX;
    assert_eq!(
        owner.remove_current_names(&["token"]),
        Err(QueryError::RevisionExhausted)
    );
    assert_eq!(
        owner.replace_raw(Some("a=2".into())),
        Err(QueryError::RevisionExhausted)
    );
    assert_eq!(owner.invalidate_projection(), Err(QueryError::RevisionExhausted));
    assert_eq!(owner.current_raw(), Some("token=x&a=1"));
}

#[test]
fn private_baseline_owner_has_reviewed_eighty_byte_layout() {
    type FormerFields = (
        Option<String>,
        std::sync::OnceLock<Option<Box<std::collections::HashMap<String, String>>>>,
    );
    assert_eq!(std::mem::size_of::<RequestQuery>(), 80);
    assert_eq!(
        std::mem::size_of::<RequestQuery>() - std::mem::size_of::<FormerFields>(),
        40
    );
}

#[test]
fn baseline_is_private_first_capture_and_restore_requires_same_owner_state() {
    let mut owner = query(Some("token=entry&a=1"));
    owner.remove_current_names(&["token"]).unwrap();
    owner.capture_baseline().unwrap();
    owner.replace_raw(Some("provider=one".into())).unwrap();
    owner.capture_baseline().unwrap();
    owner.restore_baseline().unwrap();
    assert_eq!(owner.current_raw(), Some("a=1"));
    assert_eq!(owner.entry_param("token"), Some("entry"));
    assert!(owner.projection_is_dirty());
    let mut unrelated = query(Some("other=request"));
    assert_eq!(unrelated.restore_baseline(), Err(QueryError::NoBaseline));
    assert_eq!(unrelated.current_raw(), Some("other=request"));
    owner.replace_raw(Some("provider=two".into())).unwrap();
    owner.revision = u64::MAX;
    assert_eq!(owner.restore_baseline(), Err(QueryError::RevisionExhausted));
    assert_eq!(owner.current_raw(), Some("provider=two"));
}
