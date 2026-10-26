use super::*;

#[test]
fn equality_and_empty_selection_do_not_render() {
    let document = QueryDocument::new(Some("a=1&&b=2&c=3".into()));
    assert_eq!(document.pairs().count(), 3);
    let selected = Selection::new(SelectionBase::Entry, vec![0, 2], &document).unwrap();
    assert!(selected.equals_raw(&document, Some("a=1&c=3")));
    assert!(!selected.equals_raw(&document, Some("a=1&&c=3")));
    assert!(!selected.equals_raw(&document, None));
    assert!(selected.rendered.get().is_none());
    let first = selected.raw(&document).unwrap();
    assert_eq!(first, "a=1&c=3");
    assert_eq!(first.as_ptr(), selected.raw(&document).unwrap().as_ptr());

    let empty = Selection::new(SelectionBase::Entry, vec![], &document).unwrap();
    assert!(empty.equals_raw(&document, None));
    assert!(!empty.equals_raw(&document, Some("")));
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| assert_eq!(empty.raw(&document), None));
    assert_eq!(stats.calls, 0);
}
