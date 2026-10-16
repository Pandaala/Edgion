use super::*;
use serde_json::json;
use std::cell::Cell;
use std::collections::VecDeque;
use std::future::ready;

fn expectation() -> ConflictExpectation {
    ConflictExpectation {
        target: ObservedTarget {
            group: String::new(),
            kind: "Service".into(),
            namespace: NS.into(),
            name: "ebtp-conflict-svc-current".into(),
        },
        winner: service_key("ebtp-conflict-a-win-current"),
        loser: service_key("ebtp-conflict-z-lose-current"),
    }
}

fn observation(generation: u64, candidates: &[&str], winner: Option<(&str, &str)>) -> Value {
    json!({"success":true,"data": {
        "generation":generation, "complete":true,
        "target":{"group":"","kind":"Service","namespace":NS,"name":"ebtp-conflict-svc-current"},
        "candidates":candidates,
        "effective":winner.map(|(key, algorithm)| json!({"policyKey":key,"loadBalancer":algorithm})),
    }})
}

fn parse(value: Value) -> Result<PreparedObservation, String> {
    parse_prepared_observation(reqwest::StatusCode::OK, &serde_json::to_vec(&value).unwrap())
}

#[tokio::test]
async fn prepared_conflict_delayed_dispatch_blocks_delete_until_both_are_prepared() {
    let expected = expectation();
    let queued_generic_cache = [&expected.winner, &expected.loser];
    let deleted = Cell::new(0);
    let reads = Cell::new(0);
    let mut snapshots = VecDeque::from([
        observation(1, &[&expected.winner], Some((&expected.winner, "Ewma"))),
        observation(1, &[&expected.winner], Some((&expected.winner, "Ewma"))),
        observation(
            2,
            &[&expected.winner, &expected.loser],
            Some((&expected.winner, "Ewma")),
        ),
    ]);
    let result: Result<(), String> = async {
        let conflict = poll_prepared_conflict(
            &expected,
            PreparedPhase::Conflict,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || {
                assert_eq!(deleted.get(), 0, "deletion must await the prepared barrier");
                assert_eq!(queued_generic_cache.len(), 2, "ingress alone contains both policies");
                reads.set(reads.get() + 1);
                ready(parse(snapshots.pop_front().expect("bounded scripted dispatch")))
            },
        )
        .await?;
        // The same success continuation as the live Admin delete is gated by ?.
        deleted.set(deleted.get() + 1);
        poll_prepared_conflict(
            &expected,
            PreparedPhase::Promotion {
                after_generation: conflict.generation,
            },
            tokio::time::Instant::now() + Duration::from_secs(1),
            || {
                ready(parse(observation(
                    3,
                    &[&expected.loser],
                    Some((&expected.loser, "LeastConn")),
                )))
            },
        )
        .await?;
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(reads.get(), 3);
    assert_eq!(deleted.get(), 1);
}

#[tokio::test]
async fn prepared_conflict_latest_wins_after_delayed_dispatch_never_deletes() {
    let expected = expectation();
    let deleted = Cell::new(false);
    let mut snapshots = VecDeque::from([
        observation(1, &[&expected.winner], Some((&expected.winner, "Ewma"))),
        observation(
            2,
            &[&expected.winner, &expected.loser],
            Some((&expected.loser, "LeastConn")),
        ),
    ]);
    let result: Result<(), String> = async {
        poll_prepared_conflict(
            &expected,
            PreparedPhase::Conflict,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || ready(parse(snapshots.pop_front().expect("wrong-winner must fail promptly"))),
        )
        .await?;
        deleted.set(true);
        Ok(())
    }
    .await;
    assert!(result.unwrap_err().contains("arbitration mismatch"));
    assert!(!deleted.get());
    assert!(snapshots.is_empty());
}

#[tokio::test]
async fn prepared_conflict_pending_deadline_never_deletes_or_reads_again() {
    let expected = expectation();
    let deleted = Cell::new(false);
    let reads = Cell::new(0);
    let result: Result<(), String> = async {
        poll_prepared_conflict(
            &expected,
            PreparedPhase::Conflict,
            tokio::time::Instant::now() + Duration::from_millis(5),
            || {
                reads.set(reads.get() + 1);
                ready(parse(observation(
                    1,
                    &[&expected.winner],
                    Some((&expected.winner, "Ewma")),
                )))
            },
        )
        .await?;
        deleted.set(true);
        Ok(())
    }
    .await;
    assert!(result.unwrap_err().contains("deadline expired"));
    // A heavily scheduled runtime can expire before its first read. It must
    // never issue a second read after the pending observation's deadline.
    assert!(reads.get() <= 1);
    let reads_at_deadline = reads.get();
    assert!(!deleted.get());
    let already_expired =
        poll_prepared_conflict(&expected, PreparedPhase::Conflict, tokio::time::Instant::now(), || {
            reads.set(reads.get() + 1);
            ready(Err("must not read".into()))
        })
        .await;
    assert!(already_expired.is_err());
    assert_eq!(reads.get(), reads_at_deadline);
}

#[tokio::test]
async fn prepared_conflict_reader_errors_and_complete_reader_deadlines_fail() {
    let expected = expectation();
    let error = poll_prepared_conflict(
        &expected,
        PreparedPhase::Conflict,
        tokio::time::Instant::now() + Duration::from_secs(1),
        || ready(Err("body read failed".into())),
    )
    .await
    .unwrap_err();
    assert_eq!(error, "body read failed");
    let error = poll_prepared_conflict(
        &expected,
        PreparedPhase::Conflict,
        tokio::time::Instant::now() + Duration::from_millis(100),
        std::future::pending::<Result<PreparedObservation, String>>,
    )
    .await
    .unwrap_err();
    assert!(error.contains("reader timed out"));
}

#[test]
fn prepared_conflict_current_identities_target_and_promotion_are_required() {
    let expected = expectation();
    let first = parse(observation(1, &[&expected.winner], Some((&expected.winner, "Ewma")))).unwrap();
    assert!(expected.phase_ready(&first, PreparedPhase::Winner).unwrap());
    assert!(!expected.phase_ready(&first, PreparedPhase::Conflict).unwrap());
    let old = parse(observation(
        500,
        &[
            "edgion-default/ebtp-conflict-a-win-previous",
            "edgion-default/ebtp-conflict-z-lose-previous",
        ],
        Some(("edgion-default/ebtp-conflict-a-win-previous", "Ewma")),
    ))
    .unwrap();
    assert!(!expected.phase_ready(&old, PreparedPhase::Conflict).unwrap());
    let both = parse(observation(
        2,
        &[&expected.winner, &expected.loser],
        Some((&expected.winner, "Ewma")),
    ))
    .unwrap();
    assert!(!expected
        .phase_ready(&both, PreparedPhase::Promotion { after_generation: 1 })
        .unwrap());
    let promoted = parse(observation(3, &[&expected.loser], Some((&expected.loser, "LeastConn")))).unwrap();
    assert!(!expected
        .phase_ready(&promoted, PreparedPhase::Promotion { after_generation: 3 })
        .unwrap());
    assert!(expected
        .phase_ready(&promoted, PreparedPhase::Promotion { after_generation: 2 })
        .unwrap());
    let mut wrong_target = observation(
        2,
        &[&expected.winner, &expected.loser],
        Some((&expected.winner, "Ewma")),
    );
    wrong_target["data"]["target"]["group"] = json!("edgion.io");
    wrong_target["data"]["target"]["kind"] = json!("EdgionBackend");
    assert!(expected
        .phase_ready(&parse(wrong_target).unwrap(), PreparedPhase::Conflict)
        .is_err());
}

#[test]
fn prepared_conflict_strict_parser_rejects_missing_malformed_incomplete_and_error_evidence() {
    let expected = expectation();
    let good = observation(
        2,
        &[&expected.winner, &expected.loser],
        Some((&expected.winner, "Ewma")),
    );
    for path in ["generation", "complete", "target", "candidates", "effective"] {
        let mut bad = good.clone();
        bad["data"].as_object_mut().unwrap().remove(path);
        assert!(parse(bad).is_err(), "missing {path}");
    }
    let mut cases = vec![
        json!({}),
        json!({"success":true}),
        json!({"success":false,"data":good["data"]}),
    ];
    for (pointer, value) in [
        ("/success", json!("true")),
        ("/data/complete", json!(false)),
        ("/data/generation", json!("2")),
        ("/data/candidates", json!([4])),
        ("/data/effective", Value::Null),
        ("/data/effective/policyKey", json!("other/policy")),
        ("/data/effective/loadBalancer", json!("Unknown")),
        ("/data/target/kind", json!("Unknown")),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        cases.push(bad);
    }
    let mut missing_algorithm = good.clone();
    missing_algorithm["data"]["effective"]
        .as_object_mut()
        .unwrap()
        .remove("loadBalancer");
    cases.push(missing_algorithm);
    let mut error = good.clone();
    error["error"] = json!("failed projection");
    cases.push(error);
    let mut duplicates = good.clone();
    duplicates["data"]["candidates"] = json!([expected.winner, expected.winner]);
    cases.push(duplicates);
    let mut empty_with_effective = good.clone();
    empty_with_effective["data"]["candidates"] = json!([]);
    cases.push(empty_with_effective);
    for bad in cases {
        assert!(parse(bad).is_err());
    }
    assert!(parse_prepared_observation(reqwest::StatusCode::OK, b"{").is_err());
    assert!(
        parse_prepared_observation(reqwest::StatusCode::UNAUTHORIZED, &serde_json::to_vec(&good).unwrap()).is_err()
    );
    assert!(parse(observation(0, &[], None)).is_ok());
}

#[test]
fn prepared_conflict_chunk_and_metadata_bounds_fail_before_append() {
    let mut body = vec![b' '; OBSERVATION_BODY_LIMIT - 2];
    append_observation_chunk(&mut body, b"{}").unwrap();
    assert_eq!(body.len(), OBSERVATION_BODY_LIMIT);
    let before = body.clone();
    assert!(append_observation_chunk(&mut body, b"x").is_err());
    assert_eq!(body, before, "overflow chunk must never be appended");
    assert!(parse_prepared_observation(reqwest::StatusCode::OK, &vec![b' '; OBSERVATION_BODY_LIMIT + 1]).is_err());
    let expected = expectation();
    let mut too_many = observation(1, &[], None);
    too_many["data"]["candidates"] = json!((0..33).map(|i| format!("ns/p{i}")).collect::<Vec<_>>());
    assert!(parse(too_many).is_err());
    let mut too_long = observation(1, &[&expected.winner], Some((&expected.winner, "Ewma")));
    too_long["data"]["candidates"] = json!(["x".repeat(513)]);
    assert!(parse(too_long).is_err());
}
