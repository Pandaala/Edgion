use super::*;

#[derive(Clone, Copy)]
enum ChainOrder {
    LeafFirst,
    RootFirst,
}

#[derive(Debug, PartialEq, Eq)]
struct ChainWalk {
    leaf_evaluations: usize,
    terminal_logs: usize,
    rejected: bool,
}

fn walk_verify_stages(
    cert_count: usize,
    order: ChainOrder,
    mut preverify_ok_at: impl FnMut(c_int) -> bool,
    context_present: bool,
    leaf_allowlist_ok: bool,
    stop_on_reject: bool,
) -> ChainWalk {
    assert!((1..=9).contains(&cert_count));
    let mut depths: Vec<c_int> = (0..cert_count as c_int).collect();
    if matches!(order, ChainOrder::RootFirst) {
        depths.reverse();
    }

    let audit = AtomicBool::new(false);
    let mut walk = ChainWalk {
        leaf_evaluations: 0,
        terminal_logs: 0,
        rejected: false,
    };

    for depth in depths {
        let stage = classify_verify_callback(preverify_ok_at(depth), depth, context_present);
        match stage {
            VerifyStage::RejectChain | VerifyStage::RejectMissingContext | VerifyStage::RejectInvalidDepth => {
                walk.rejected = true;
                if claim_terminal_audit(&audit) {
                    walk.terminal_logs += 1;
                }
                if stop_on_reject {
                    break;
                }
            }
            VerifyStage::AcceptIntermediate => {}
            VerifyStage::EvaluateLeafAllowlist => {
                walk.leaf_evaluations += 1;
                if !leaf_allowlist_ok {
                    walk.rejected = true;
                    if claim_terminal_audit(&audit) {
                        walk.terminal_logs += 1;
                    }
                    if stop_on_reject {
                        break;
                    }
                }
            }
        }
    }
    walk
}

#[test]
fn classify_rejects_failed_preverify_at_every_depth() {
    for depth in 0..=8 {
        assert_eq!(classify_verify_callback(false, depth, true), VerifyStage::RejectChain);
        assert_eq!(classify_verify_callback(false, depth, false), VerifyStage::RejectChain);
    }
}

#[test]
fn classify_accepts_intermediate_without_context() {
    for depth in 1..=8 {
        assert_eq!(
            classify_verify_callback(true, depth, false),
            VerifyStage::AcceptIntermediate
        );
        assert_eq!(
            classify_verify_callback(true, depth, true),
            VerifyStage::AcceptIntermediate
        );
    }
}

#[test]
fn classify_leaf_requires_context_and_evaluates_allowlist() {
    assert_eq!(
        classify_verify_callback(true, 0, true),
        VerifyStage::EvaluateLeafAllowlist
    );
    assert_eq!(
        classify_verify_callback(true, 0, false),
        VerifyStage::RejectMissingContext
    );
}

#[test]
fn classify_negative_depth_is_fail_closed() {
    assert_eq!(
        classify_verify_callback(true, -1, true),
        VerifyStage::RejectInvalidDepth
    );
}

#[test]
fn successful_chains_evaluate_leaf_once_for_one_through_nine_certs() {
    for cert_count in 1..=9 {
        for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
            let walk = walk_verify_stages(cert_count, order, |_| true, true, true, true);
            assert_eq!(
                walk,
                ChainWalk {
                    leaf_evaluations: 1,
                    terminal_logs: 0,
                    rejected: false,
                },
                "cert_count={cert_count}"
            );
        }
    }
}

#[test]
fn preverify_failure_rejects_immediately_without_leaf_work() {
    for cert_count in 1..=9 {
        for fail_depth in 0..cert_count as c_int {
            let walk = walk_verify_stages(
                cert_count,
                ChainOrder::RootFirst,
                |depth| depth != fail_depth,
                true,
                true,
                true,
            );
            assert!(walk.rejected, "fail_depth={fail_depth}");
            assert_eq!(walk.terminal_logs, 1, "fail_depth={fail_depth}");
            assert_eq!(walk.leaf_evaluations, 0, "fail_depth={fail_depth}");
        }
    }
}

#[test]
fn missing_leaf_context_is_fail_closed() {
    let walk = walk_verify_stages(3, ChainOrder::LeafFirst, |_| true, false, true, true);
    assert_eq!(
        walk,
        ChainWalk {
            leaf_evaluations: 0,
            terminal_logs: 1,
            rejected: true,
        }
    );
}

#[test]
fn intermediate_success_does_not_extract_or_log() {
    let walk = walk_verify_stages(4, ChainOrder::RootFirst, |_| true, true, true, true);
    assert_eq!(walk.leaf_evaluations, 1);
    assert_eq!(walk.terminal_logs, 0);
    assert!(!walk.rejected);
}

#[test]
fn allowlist_failure_emits_one_terminal_log() {
    for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
        let walk = walk_verify_stages(5, order, |_| true, true, false, false);
        assert_eq!(
            walk,
            ChainWalk {
                leaf_evaluations: 1,
                terminal_logs: 1,
                rejected: true,
            }
        );
    }
}

#[test]
fn terminal_audit_is_claimed_once_across_repeated_failures() {
    let flag = AtomicBool::new(false);
    assert!(claim_terminal_audit(&flag));
    assert!(!claim_terminal_audit(&flag));
    assert!(!claim_terminal_audit(&flag));
}
