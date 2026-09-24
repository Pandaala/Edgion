use super::*;

fn k(kind: &str, key: &str) -> ResourceKey {
    ResourceKey::new(kind, key)
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn record_new_key_returns_new_winner() {
    let r = PathRegistry::new();
    let outcome = r.record(k("Service", "default/x"), p("/conf/a.yaml"));
    assert_eq!(
        outcome,
        RecordOutcome::NewWinner {
            path: p("/conf/a.yaml")
        }
    );
    assert_eq!(r.winner_of(&k("Service", "default/x")), Some(p("/conf/a.yaml")));
}

#[test]
fn replace_from_atomically_installs_a_new_scan_generation() {
    let current = PathRegistry::new();
    let key = k("Service", "default/x");
    current.record(key.clone(), p("/old/service.yaml"));
    let next = PathRegistry::new();
    let new_path = p("/new/service.yaml");
    next.record(key.clone(), new_path.clone());

    current.replace_from(&next);

    assert_eq!(current.winner_of(&key), Some(new_path));
    assert!(current.snapshot_conflicts().is_empty());
}

#[test]
fn record_second_path_returns_conflict_winner_unchanged_regardless_of_lex_order() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    let outcome = r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    assert_eq!(
        outcome,
        RecordOutcome::ConflictAdded {
            winner: p("/conf/b.yaml"),
            conflicts: vec![p("/conf/a.yaml")]
        }
    );
    assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/conf/b.yaml")));
}

#[test]
fn record_known_path_returns_already_known_with_correct_is_winner() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    assert_eq!(
        r.record(k("S", "ns/x"), p("/conf/a.yaml")),
        RecordOutcome::AlreadyKnown { is_winner: true }
    );
    assert_eq!(
        r.record(k("S", "ns/x"), p("/conf/b.yaml")),
        RecordOutcome::AlreadyKnown { is_winner: false }
    );
}

#[test]
fn forget_winner_with_conflicts_promotes_first_inserted() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/c.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    let outcome = r.forget(&p("/conf/a.yaml"));
    assert_eq!(
        outcome,
        ForgetOutcome::Promoted {
            new_winner: p("/conf/c.yaml"),
            remaining_conflicts: vec![p("/conf/b.yaml")]
        }
    );
}

#[test]
fn forget_non_winner_returns_conflict_removed() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    assert_eq!(
        r.forget(&p("/conf/b.yaml")),
        ForgetOutcome::ConflictRemoved {
            winner: p("/conf/a.yaml"),
            remaining_conflicts: vec![]
        }
    );
}

#[test]
fn forget_unknown_path_returns_unknown() {
    let r = PathRegistry::new();
    assert_eq!(r.forget(&p("/nope")), ForgetOutcome::Unknown);
}

#[test]
fn forget_only_path_returns_removed() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    assert_eq!(
        r.forget(&p("/conf/a.yaml")),
        ForgetOutcome::Removed { key: k("S", "ns/x") }
    );
    assert_eq!(r.winner_of(&k("S", "ns/x")), None);
}

#[test]
fn mark_api_deleted_hides_winner_from_winner_of() {
    let r = PathRegistry::new();
    r.record(k("Gateway", "ns/gw"), p("/conf/gw.yaml"));
    r.mark_api_deleted(&p("/conf/gw.yaml"));
    assert_eq!(r.winner_of(&k("Gateway", "ns/gw")), None);
}

#[test]
fn forget_after_mark_api_deleted_returns_removed_and_clears_marker() {
    let r = PathRegistry::new();
    r.record(k("Gateway", "ns/gw"), p("/conf/gw.yaml"));
    r.mark_api_deleted(&p("/conf/gw.yaml"));
    // watcher fires Remove — must still get Removed so Delete event is dispatched
    assert_eq!(
        r.forget(&p("/conf/gw.yaml")),
        ForgetOutcome::Removed {
            key: k("Gateway", "ns/gw")
        }
    );
    // pending marker gone, winner_of is also gone
    assert_eq!(r.winner_of(&k("Gateway", "ns/gw")), None);
}

#[test]
fn mark_api_deleted_winner_with_conflict_hides_winner_but_exposes_conflict() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/a.yaml")); // winner
    r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
    r.mark_api_deleted(&p("/a.yaml"));
    // conflict file is still live — winner_of must surface it
    assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/b.yaml")));
}

#[test]
fn forget_after_mark_api_deleted_winner_with_conflict_promotes() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/a.yaml"));
    r.record(k("S", "ns/x"), p("/b.yaml"));
    r.mark_api_deleted(&p("/a.yaml"));
    // watcher fires Remove for /a.yaml — must still return Promoted
    assert_eq!(
        r.forget(&p("/a.yaml")),
        ForgetOutcome::Promoted {
            new_winner: p("/b.yaml"),
            remaining_conflicts: vec![]
        }
    );
}

#[test]
fn mark_api_deleted_unknown_path_is_noop() {
    let r = PathRegistry::new();
    r.mark_api_deleted(&p("/nonexistent.yaml")); // must not panic
    assert_eq!(r.winner_of(&k("S", "ns/x")), None);
}

#[test]
fn snapshot_conflicts_only_lists_keys_with_more_than_one_path() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    r.record(k("S", "ns/y"), p("/conf/y.yaml"));
    let snap = r.snapshot_conflicts();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].key, k("S", "ns/x"));
    assert_eq!(snap[0].winner, p("/conf/a.yaml"));
    assert_eq!(snap[0].losers, vec![p("/conf/b.yaml")]);
}

#[test]
fn snapshot_winners_returns_all_keys_with_their_winner() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    r.record(k("HTTPRoute", "ns/y"), p("/conf/y.yaml"));
    let mut snap = r.snapshot_winners();
    snap.sort_by(|a, b| a.0.key.cmp(&b.0.key));
    assert_eq!(snap.len(), 2);
}

// --- snapshot_conflicts pending-aware tests ---

#[test]
fn snapshot_conflicts_pending_winner_with_one_live_conflict_not_reported() {
    // winner pending + 1 live conflict → live.len() == 1 → not a conflict
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/a.yaml")); // winner
    r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
    r.mark_api_deleted(&p("/a.yaml"));
    let snap = r.snapshot_conflicts();
    assert!(snap.is_empty(), "one live path remaining is not a conflict");
}

#[test]
fn snapshot_conflicts_pending_winner_with_two_live_conflicts_reports_correctly() {
    // winner pending + 2 live conflicts → live winner = first live conflict
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/a.yaml")); // winner
    r.record(k("S", "ns/x"), p("/b.yaml")); // conflict 1
    r.record(k("S", "ns/x"), p("/c.yaml")); // conflict 2
    r.mark_api_deleted(&p("/a.yaml"));
    let snap = r.snapshot_conflicts();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].winner, p("/b.yaml"));
    assert_eq!(snap[0].losers, vec![p("/c.yaml")]);
}

// --- snapshot_winners pending-aware tests ---

#[test]
fn snapshot_winners_excludes_key_when_all_paths_pending() {
    let r = PathRegistry::new();
    r.record(k("Gateway", "ns/gw"), p("/gw.yaml"));
    r.record(k("S", "ns/x"), p("/x.yaml"));
    r.mark_api_deleted(&p("/gw.yaml")); // only path for ns/gw is pending
    let snap = r.snapshot_winners();
    // ns/gw has no live winner — must not appear
    assert!(
        snap.iter().all(|(rk, _)| rk != &k("Gateway", "ns/gw")),
        "pending-only key must be excluded from snapshot_winners"
    );
    // ns/x is untouched — must still appear
    assert!(snap.iter().any(|(rk, _)| rk == &k("S", "ns/x")));
}

#[test]
fn snapshot_winners_consistent_with_winner_of_under_pending() {
    // For every key, snapshot_winners and winner_of must agree
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/a.yaml")); // winner
    r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
    r.record(k("S", "ns/y"), p("/y.yaml"));
    r.mark_api_deleted(&p("/a.yaml")); // ns/x winner is pending

    let snap: std::collections::HashMap<_, _> = r.snapshot_winners().into_iter().collect();

    // ns/x: snapshot_winners and winner_of must both return /b.yaml
    assert_eq!(snap.get(&k("S", "ns/x")), Some(&p("/b.yaml")));
    assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/b.yaml")));

    // ns/y: unaffected — both return /y.yaml
    assert_eq!(snap.get(&k("S", "ns/y")), Some(&p("/y.yaml")));
    assert_eq!(r.winner_of(&k("S", "ns/y")), Some(p("/y.yaml")));
}

// --- is_recorded_loser tests ---

#[test]
fn is_recorded_loser_unknown_path_is_false() {
    let r = PathRegistry::new();
    assert!(!r.is_recorded_loser(&p("/conf/x.yaml")));
}

#[test]
fn is_recorded_loser_winner_false_loser_true() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml")); // winner (first)
    r.record(k("S", "ns/x"), p("/conf/b.yaml")); // loser
    assert!(!r.is_recorded_loser(&p("/conf/a.yaml")), "winner is not a loser");
    assert!(r.is_recorded_loser(&p("/conf/b.yaml")), "second path is a loser");
}

#[test]
fn is_recorded_loser_pending_winner_promotes_loser() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.record(k("S", "ns/x"), p("/conf/b.yaml"));
    r.mark_api_deleted(&p("/conf/a.yaml")); // a pending -> b is the live winner
    assert!(
        !r.is_recorded_loser(&p("/conf/b.yaml")),
        "promoted loser must no longer be flagged a loser (pending-aware)"
    );
}

#[test]
fn is_recorded_loser_all_pending_is_false() {
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml"));
    r.mark_api_deleted(&p("/conf/a.yaml")); // no live winner left
    assert!(!r.is_recorded_loser(&p("/conf/a.yaml")), "no live winner -> keep");
}

#[test]
fn is_recorded_loser_all_pending_two_paths_is_false() {
    // winner + loser BOTH pending -> no live winner -> neither path is a loser
    // (exercises the `None => false` branch with more than one recorded path).
    let r = PathRegistry::new();
    r.record(k("S", "ns/x"), p("/conf/a.yaml")); // winner
    r.record(k("S", "ns/x"), p("/conf/b.yaml")); // loser
    r.mark_api_deleted(&p("/conf/a.yaml"));
    r.mark_api_deleted(&p("/conf/b.yaml"));
    assert!(
        !r.is_recorded_loser(&p("/conf/a.yaml")),
        "all paths pending -> keep (no live winner)"
    );
    assert!(
        !r.is_recorded_loser(&p("/conf/b.yaml")),
        "all paths pending -> keep (no live winner)"
    );
}
