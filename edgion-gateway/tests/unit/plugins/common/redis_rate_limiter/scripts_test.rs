use super::TOKEN_BUCKET_SCRIPT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BucketState {
    tokens: u64,
    last_ts: i64,
    refill_remainder: u64,
}

fn attempt(
    state: Option<(u64, i64, Option<u64>)>,
    max_tokens: u64,
    interval_ms: u64,
    now: i64,
    cost: u64,
) -> (BucketState, bool, u64) {
    let (mut tokens, mut last_ts, mut refill_remainder) = state
        .map(|(tokens, last_ts, remainder)| (tokens, last_ts, remainder.unwrap_or(0)))
        .unwrap_or((max_tokens, now, 0));

    if now >= last_ts {
        let elapsed = (now - last_ts) as u64;
        if elapsed >= interval_ms {
            tokens = max_tokens;
            refill_remainder = 0;
            last_ts = now;
        } else {
            let refill_numerator = elapsed * max_tokens + refill_remainder;
            let refill = refill_numerator / interval_ms;
            refill_remainder = refill_numerator - refill * interval_ms;
            tokens += refill;
            last_ts = now;
            if tokens >= max_tokens {
                tokens = max_tokens;
                refill_remainder = 0;
            }
        }
    }

    let allowed = tokens >= cost;
    let retry_ms = if allowed {
        tokens -= cost;
        0
    } else {
        let required_numerator = (cost - tokens) * interval_ms - refill_remainder;
        (required_numerator - 1) / max_tokens + 1
    };
    (
        BucketState {
            tokens,
            last_ts,
            refill_remainder,
        },
        allowed,
        retry_ms,
    )
}

#[test]
fn token_bucket_persists_complete_state_for_allow_and_deny() {
    const PERSIST_STATE: &str =
        "redis.call('HMSET', key, 'tokens', tokens, 'ts', last_ts, 'refill_remainder', refill_remainder)";
    const ALLOW_TO_DENY_BOUNDARY: &str = "    return {1, tokens, tokens, 0}\nelse\n";
    const DENY_RETURN_AND_END: &str = "    return {0, tokens, 0, retry_ms}\nend";

    let (through_allow_branch, deny_branch_and_tail) = TOKEN_BUCKET_SCRIPT
        .split_once(ALLOW_TO_DENY_BOUNDARY)
        .expect("token-bucket script must contain the outer allow-return/deny boundary");
    let allow_branch = through_allow_branch
        .rsplit_once("if tokens >= cost then")
        .expect("token-bucket script must contain the outer token-balance branch")
        .1;
    let deny_branch = deny_branch_and_tail
        .split_once(DENY_RETURN_AND_END)
        .expect("token-bucket script must contain the outer deny return and end")
        .0;

    assert!(allow_branch.contains(PERSIST_STATE));
    assert!(deny_branch.contains(PERSIST_STATE));
    assert_eq!(TOKEN_BUCKET_SCRIPT.matches(PERSIST_STATE).count(), 2);
}

#[test]
fn token_bucket_script_carries_fractional_refill_and_uses_integer_retry_ceiling() {
    for contract in [
        "redis.call('HMGET', key, 'tokens', 'ts', 'refill_remainder')",
        "local refill_remainder = tonumber(data[3]) or 0",
        "if now >= last_ts then",
        "if elapsed >= interval_ms then",
        "local refill_numerator = elapsed * refill_rate + refill_remainder",
        "refill_remainder = refill_numerator - refill * interval_ms",
        "local required_numerator = needed * interval_ms - refill_remainder",
        "local retry_ms = math.floor((required_numerator - 1) / refill_rate) + 1",
    ] {
        assert!(
            TOKEN_BUCKET_SCRIPT.contains(contract),
            "missing Lua contract: {contract}"
        );
    }
}

#[test]
fn token_bucket_fractional_refill_survives_allow_and_deny_branches() {
    let (drained, allowed, retry_ms) = attempt(None, 3, 1_000, 0, 3);
    assert!(allowed);
    assert_eq!(retry_ms, 0);
    assert_eq!(drained.tokens, 0);

    let (at_500, allowed, retry_ms) = attempt(Some((0, 0, Some(0))), 3, 1_000, 500, 2);
    assert!(!allowed);
    assert_eq!(retry_ms, 167);
    assert_eq!((at_500.tokens, at_500.refill_remainder), (1, 500));

    let (at_1_000, allowed, _) = attempt(
        Some((at_500.tokens, at_500.last_ts, Some(at_500.refill_remainder))),
        3,
        1_000,
        1_000,
        2,
    );
    assert!(allowed);
    assert_eq!((at_1_000.tokens, at_1_000.refill_remainder), (1, 0));

    let (at_1_500, allowed, _) = attempt(
        Some((at_1_000.tokens, at_1_000.last_ts, Some(at_1_000.refill_remainder))),
        3,
        1_000,
        1_500,
        2,
    );
    assert!(allowed);
    assert_eq!((at_1_500.tokens, at_1_500.refill_remainder), (0, 500));

    let (at_2_000, allowed, _) = attempt(
        Some((at_1_500.tokens, at_1_500.last_ts, Some(at_1_500.refill_remainder))),
        3,
        1_000,
        2_000,
        2,
    );
    assert!(allowed);
    assert_eq!((at_2_000.tokens, at_2_000.refill_remainder), (0, 0));
}

#[test]
fn token_bucket_migrates_saturates_and_handles_clock_rollback() {
    let (migrated, allowed, retry_ms) = attempt(Some((0, 0, None)), 3, 1_000, 500, 2);
    assert!(!allowed);
    assert_eq!(retry_ms, 167);
    assert_eq!((migrated.tokens, migrated.refill_remainder), (1, 500));

    let (saturated, allowed, _) = attempt(Some((2, 0, Some(900))), 3, 1_000, 100, 1);
    assert!(allowed);
    assert_eq!(
        saturated,
        BucketState {
            tokens: 2,
            last_ts: 100,
            refill_remainder: 0,
        }
    );

    let (full_interval, allowed, _) = attempt(Some((0, 0, Some(999))), 3, 1_000, i64::MAX, 1);
    assert!(allowed);
    assert_eq!(
        full_interval,
        BucketState {
            tokens: 2,
            last_ts: i64::MAX,
            refill_remainder: 0,
        }
    );

    let (rolled_back, allowed, _) = attempt(Some((1, 1_000, Some(500))), 3, 1_000, 900, 1);
    assert!(allowed);
    assert_eq!(
        rolled_back,
        BucketState {
            tokens: 0,
            last_ts: 1_000,
            refill_remainder: 500,
        }
    );

    let (_, allowed, retry_ms) = attempt(Some((0, 1_000, Some(999))), 3, 1_000, 1_000, 1);
    assert!(!allowed);
    assert_eq!(retry_ms, 1);
}
