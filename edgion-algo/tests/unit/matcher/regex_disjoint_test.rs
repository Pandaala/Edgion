use super::*;
#[test]
fn anchored_languages_include_eoi_unicode_and_optional_tails() {
    for patterns in [
        vec!["^/a/[^/]+$", "^/a/literal$"],
        vec!["^/a(?:/.*)?$", "^/a$"],
        vec!["^/é/[^/]+:watch$", "^/é/π:watch$"],
        vec!["^$", "^$"],
    ] {
        assert_eq!(check_disjoint(&[patterns]), Err(DisjointError::Overlap));
    }
    assert_eq!(check_disjoint(&[vec!["^/a$", "^/ab$", "^/é$", "^/a:watch$"]]), Ok(()));
    assert_eq!(check_disjoint(&[vec!["^/a$"], vec!["^/a$"]]), Ok(()));
}
#[test]
fn all_publication_allowances_fail_explicitly() {
    let patterns = [vec!["^/alpha$", "^/beta$"]];
    for (limits, error) in [
        (Limits { patterns: 1, ..LIMITS }, DisjointError::InputLimit),
        (Limits { input: 1, ..LIMITS }, DisjointError::InputLimit),
        (Limits { nfa: 1, ..LIMITS }, DisjointError::BuildLimit),
        (Limits { dfa: 1, ..LIMITS }, DisjointError::BuildLimit),
        (Limits { scratch: 1, ..LIMITS }, DisjointError::BuildLimit),
        (Limits { states: 1, ..LIMITS }, DisjointError::StateLimit),
        (
            Limits {
                transitions: 1,
                ..LIMITS
            },
            DisjointError::WorkLimit,
        ),
    ] {
        assert_eq!(check(&patterns, limits), Err(error));
    }
    let single = [vec!["^/a$"]];
    let minimum = (1..5000)
        .find(|&n| {
            check(
                &single,
                Limits {
                    transitions: n,
                    ..LIMITS
                },
            )
            .is_ok()
        })
        .unwrap();
    assert_eq!(
        check(
            &[vec!["^/a$"], vec!["^/b$"]],
            Limits {
                transitions: minimum,
                ..LIMITS
            }
        ),
        Err(DisjointError::WorkLimit)
    );
}
