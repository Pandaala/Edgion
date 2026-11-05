use super::*;
use std::collections::HashMap;

#[test]
fn script_keys_keep_owned_bytes_without_intermediate_storage() {
    let keys = ["first".to_owned(), "second".to_owned()];
    let pointers = keys.each_ref().map(|key| key.as_ptr());
    let result = script_keys(keys);
    assert_eq!(result, ["first", "second"]);
    assert_eq!(result[0].as_ptr(), pointers[0]);
    assert_eq!(result[1].as_ptr(), pointers[1]);
}
#[test]
fn script_arguments_keep_integer_order_without_intermediate_storage() {
    let args = [i64::MIN, -1, 0, 1, i64::MAX];
    assert_eq!(script_args(args), args);
}

#[test]
fn script_descriptors_pair_each_source_with_its_redis_sha1() {
    let core = RedisRateLimiter::new();
    let cases = [
        (&core.scripts.sliding_window, SLIDING_WINDOW_SCRIPT),
        (&core.scripts.fixed_window, FIXED_WINDOW_SCRIPT),
        (&core.scripts.token_bucket, TOKEN_BUCKET_SCRIPT),
        (&core.scripts.cumulative_quota, CUMULATIVE_QUOTA_SCRIPT),
        (&core.scripts.credential_reservation, CREDENTIAL_RESERVATION_SCRIPT),
    ];
    for (script, source) in cases {
        assert_eq!(script.source, source);
        assert_eq!(script.sha1, format!("{:x}", Sha1::digest(source.as_bytes())));
    }
    // Redis's documented SCRIPT LOAD example anchors the digest's wire format.
    assert_eq!(
        ScriptDescriptor::new("return 'Immabe a cached script'").sha1,
        "c664a3bf70bd1d45c4284ffebb65a6f2299bfc9f"
    );
}

#[test]
fn new_limiters_reuse_the_initialized_script_descriptors() {
    let first = RedisRateLimiter::new();
    let second = RedisRateLimiter::default();
    assert!(std::ptr::eq(first.scripts, second.scripts));
}

#[test]
fn only_a_redis_noscript_error_permits_fallback() {
    let missing = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::NoScript), "missing"));
    assert!(is_missing_script(&missing));
    for message in ["ERR NOSCRIPT", "WRONGTYPE NOSCRIPT", "NOSCRIPT"] {
        assert!(!is_missing_script(&redis::RedisError::from((
            redis::ErrorKind::Client,
            message
        ))));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryMarker {
    Reserved { rpm: u64, tpm: u64 },
    Reconciled(u64),
    Released,
}

#[derive(Default)]
struct MemoryCredentialLedger {
    rpm_used: u64,
    tpm_used: u64,
    markers: HashMap<String, MemoryMarker>,
}

impl MemoryCredentialLedger {
    fn result(
        &self,
        status: CredentialReservationStatus,
        limits: CredentialReservationLimits,
        charged_tpm: u64,
        applied_tpm_delta: i64,
    ) -> CredentialReservationResult {
        CredentialReservationResult {
            status,
            rpm_used: self.rpm_used,
            tpm_used: self.tpm_used,
            rpm_remaining: limits.rpm.map(|limit| limit.saturating_sub(self.rpm_used)),
            tpm_remaining: limits.tpm.map(|limit| limit.saturating_sub(self.tpm_used)),
            charged_tpm,
            applied_tpm_delta,
        }
    }

    fn apply(
        &mut self,
        request_id: &str,
        limits: CredentialReservationLimits,
        operation: CredentialReservationOperation,
    ) -> CredentialReservationResult {
        match operation {
            CredentialReservationOperation::Reserve { .. } => {
                if let Some(marker) = self.markers.get(request_id) {
                    let charged = match marker {
                        MemoryMarker::Reserved { tpm: tokens, .. } | MemoryMarker::Reconciled(tokens) => *tokens,
                        MemoryMarker::Released => 0,
                    };
                    return self.result(CredentialReservationStatus::Duplicate, limits, charged, 0);
                }
                let amount = if limits.tpm.is_some() {
                    operation.amount().unwrap()
                } else {
                    0
                };
                if limits.rpm.is_some_and(|limit| self.rpm_used >= limit) {
                    return self.result(CredentialReservationStatus::DeniedRpm, limits, 0, 0);
                }
                if limits
                    .tpm
                    .is_some_and(|limit| self.tpm_used > limit || amount > limit - self.tpm_used)
                {
                    return self.result(CredentialReservationStatus::DeniedTpm, limits, 0, 0);
                }
                let reserved_rpm = u64::from(limits.rpm.is_some());
                let reserved_tpm = if limits.tpm.is_some() { amount } else { 0 };
                self.rpm_used += reserved_rpm;
                self.tpm_used += reserved_tpm;
                self.markers.insert(
                    request_id.to_string(),
                    MemoryMarker::Reserved {
                        rpm: reserved_rpm,
                        tpm: reserved_tpm,
                    },
                );
                self.result(
                    CredentialReservationStatus::Reserved,
                    limits,
                    reserved_tpm,
                    i64::try_from(reserved_tpm).unwrap(),
                )
            }
            CredentialReservationOperation::Reconcile { charged_tokens } => {
                match self.markers.get(request_id).copied() {
                    Some(MemoryMarker::Reserved { tpm: reserved, .. }) => {
                        let delta = i128::from(charged_tokens) - i128::from(reserved);
                        if delta >= 0 {
                            self.tpm_used += delta as u64;
                        } else {
                            self.tpm_used -= (-delta) as u64;
                        }
                        self.markers
                            .insert(request_id.to_string(), MemoryMarker::Reconciled(charged_tokens));
                        self.result(
                            CredentialReservationStatus::Reconciled,
                            limits,
                            charged_tokens,
                            delta as i64,
                        )
                    }
                    Some(MemoryMarker::Reconciled(existing)) if existing == charged_tokens => {
                        self.result(CredentialReservationStatus::Duplicate, limits, existing, 0)
                    }
                    Some(MemoryMarker::Reconciled(existing)) => {
                        self.result(CredentialReservationStatus::Conflict, limits, existing, 0)
                    }
                    Some(MemoryMarker::Released) | None => {
                        self.result(CredentialReservationStatus::Conflict, limits, 0, 0)
                    }
                }
            }
            CredentialReservationOperation::ReleasePredispatch => match self.markers.get(request_id).copied() {
                Some(MemoryMarker::Reserved { rpm, tpm: reserved }) => {
                    self.rpm_used -= rpm;
                    self.tpm_used -= reserved;
                    self.markers.insert(request_id.to_string(), MemoryMarker::Released);
                    self.result(
                        CredentialReservationStatus::Released,
                        limits,
                        0,
                        -i64::try_from(reserved).unwrap(),
                    )
                }
                Some(MemoryMarker::Released) => self.result(CredentialReservationStatus::Duplicate, limits, 0, 0),
                Some(MemoryMarker::Reconciled(charged)) => {
                    self.result(CredentialReservationStatus::Conflict, limits, charged, 0)
                }
                None => self.result(CredentialReservationStatus::Conflict, limits, 0, 0),
            },
        }
    }
}

#[test]
fn from_lua_result_parses_fields() {
    let r = RateLimitResult::from_lua_result(&[1, 7, 3, 0], 10).unwrap();
    assert!(r.allowed);
    assert_eq!(r.current, 7);
    assert_eq!(r.remaining, 3);
    assert_eq!(r.limit, 10);
}

#[test]
fn merge_denies_when_any_denies() {
    let allow = RateLimitResult::from_lua_result(&[1, 1, 9, 0], 10).unwrap();
    let deny = RateLimitResult::from_lua_result(&[0, 11, 0, 500], 10).unwrap();
    let merged = RateLimitResult::merge(Some(allow), deny);
    assert!(!merged.allowed);
    assert_eq!(merged.retry_ms, 500);
}

#[test]
fn request_rate_reply_rejects_malformed_or_inexact_values() {
    assert!(RateLimitResult::from_lua_result(&[1, 1, 9, 0], 1_000_001).is_err());
    assert!(RateLimitResult::from_lua_result(&[1, 1, 9], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[2, 1, 9, 0], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[1, -1, 9, 0], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[1, 1, 11, 0], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[1, 1, 9, 1], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[0, 10, 1, 1], 10).is_err());
    assert!(RateLimitResult::from_lua_result(&[0, MAX_SAFE_LUA_INTEGER as i64 + 1, 0, 1], 10,).is_err());
}

#[test]
fn request_rate_execution_domain_rejects_bypassed_inputs_before_io() {
    assert!(
        validate_rate_limit_execution(1_000_000, i64::from(i32::MAX), MAX_SAFE_LUA_INTEGER as i64, 1_000_000,).is_ok()
    );
    assert!(validate_rate_limit_execution(1_000_001, 100, 0, 1).is_err());
    assert!(validate_rate_limit_execution(1, i64::from(i32::MAX) + 1, 0, 1).is_err());
    assert!(validate_rate_limit_execution(1, 100, MAX_SAFE_LUA_INTEGER as i64 + 1, 1).is_err());
    assert!(validate_rate_limit_execution(1, 100, 0, 1_000_001).is_err());

    let policies = [
        RateLimitPolicy {
            rate: 1,
            interval: String::new(),
            algorithm: RateLimitAlgorithm::FixedWindow,
            interval_duration: Some(Duration::from_millis(100)),
        },
        RateLimitPolicy {
            rate: 1_000_001,
            interval: String::new(),
            algorithm: RateLimitAlgorithm::FixedWindow,
            interval_duration: Some(Duration::from_millis(100)),
        },
    ];
    assert!(validate_rate_limit_batch(&policies, 1, 0).is_err());
}

#[test]
fn build_key_with_and_without_suffix() {
    assert_eq!(build_redis_key("p:", "sw", 0, "ip:1:2", ""), "p:sw:0:ip:1:2");
    assert_eq!(build_redis_key("p:", "fw", 1, "lst:80", "42"), "p:fw:1:lst:80:42");
}

#[test]
fn truncate_respects_utf8_boundary() {
    assert_eq!(truncate_limit_key("abcdef", 3), "abc");
    assert_eq!(truncate_limit_key("ab", 10), "ab");
}

#[test]
fn cumulative_quota_result_accepts_signed_final_correction() {
    let result = CumulativeQuotaResult::from_lua_result(&[2, -40, 60, 40, 3], 100).unwrap();
    assert_eq!(result.status, CumulativeQuotaStatus::Applied);
    assert_eq!(result.applied_delta, -40);
    assert_eq!(result.used, 60);
    assert_eq!(result.remaining, 40);
    assert_eq!(result.revision, 3);
}

#[test]
fn cumulative_quota_result_maps_every_status() {
    let cases = [
        (0, CumulativeQuotaStatus::Admitted),
        (1, CumulativeQuotaStatus::Denied),
        (2, CumulativeQuotaStatus::Applied),
        (3, CumulativeQuotaStatus::Duplicate),
        (4, CumulativeQuotaStatus::Stale),
        (5, CumulativeQuotaStatus::Conflict),
    ];
    for (code, expected) in cases {
        let result = CumulativeQuotaResult::from_lua_result(&[code, 0, 7, 3, 2], 10).unwrap();
        assert_eq!(result.status, expected);
    }
}

#[test]
fn cumulative_quota_result_strictly_rejects_invalid_replies() {
    assert!(CumulativeQuotaResult::from_lua_result(&[2, 1, 1, 9], 10).is_err());
    assert!(CumulativeQuotaResult::from_lua_result(&[6, 0, 0, 10, 0], 10).is_err());
    assert!(CumulativeQuotaResult::from_lua_result(&[2, 1, -1, 10, 0], 10).is_err());
    assert!(CumulativeQuotaResult::from_lua_result(&[2, 1, 1, 8, 0], 10).is_err());
    assert!(CumulativeQuotaResult::from_lua_result(&[3, 1, 1, 9, 0], 10).is_err());
    assert!(CumulativeQuotaResult::from_lua_result(
        &[2, 0, MAX_SAFE_LUA_INTEGER as i64 + 1, 0, 0],
        MAX_SAFE_LUA_INTEGER,
    )
    .is_err());
}

#[test]
fn cumulative_quota_enforces_lua_exact_integer_boundary() {
    assert!(validate_safe_lua_integer("test", MAX_SAFE_LUA_INTEGER).is_ok());
    assert!(validate_safe_lua_integer("test", MAX_SAFE_LUA_INTEGER + 1).is_err());
}

#[test]
fn cumulative_quota_script_uses_bounded_bucket_and_marker_keys() {
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("redis.call('HGET', key, 'used')"));
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("redis.call('HMGET', marker_key"));
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("local delta = target - previous_target"));
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("redis.call('PEXPIRE', key, bucket_ttl_ms)"));
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("redis.call('PEXPIRE', marker_key, marker_ttl_ms)"));
    assert!(CUMULATIVE_QUOTA_SCRIPT.contains("KEYS[2]"));
    assert!(!CUMULATIVE_QUOTA_SCRIPT.contains("request_field"));
}

#[test]
fn cumulative_quota_keys_require_one_explicit_cluster_slot() {
    assert!(validate_quota_key_pair("quota:{digest}:window:7", "quota:{digest}:request:9").is_ok());
    assert!(validate_quota_key_pair("quota:window:7", "quota:request:9").is_err());
    assert!(validate_quota_key_pair("quota:{a}:window:7", "quota:{b}:request:9").is_err());
    assert!(validate_quota_key_pair("quota:{same}:window:7", "quota:{same}:window:7").is_err());
}

#[test]
fn cumulative_quota_key_builder_hides_raw_identities_and_shares_slot() {
    let keys = build_cumulative_quota_keys(
        "edgion:quota:",
        b"policy-a/team-sensitive/model-a",
        42,
        b"internal-request-123",
    )
    .unwrap();
    assert!(!keys.bucket.contains("team-sensitive"));
    assert!(!keys.marker.contains("internal-request-123"));
    assert!(keys.bucket.ends_with(":window:42"));
    assert!(validate_quota_key_pair(&keys.bucket, &keys.marker).is_ok());
    assert!(build_cumulative_quota_keys("bad{prefix", b"scope", 1, b"request").is_err());
}

#[test]
fn cumulative_quota_ttls_are_independently_bounded() {
    assert_eq!(validate_quota_ttl("bucket", Duration::from_millis(1)).unwrap(), 1);
    assert!(validate_quota_ttl("marker", Duration::ZERO).is_err());
    assert_eq!(
        validate_quota_ttl("marker", Duration::from_millis(MAX_SAFE_LUA_INTEGER + 1))
            .unwrap_err()
            .to_string(),
        "marker TTL milliseconds exceeds the Redis Lua exact-integer limit"
    );
    let (result, calls) = crate::test_allocation_counter::count_allocations(|| {
        validate_quota_ttls(Duration::from_secs(10), Duration::from_secs(5))
    });
    assert_eq!(result.unwrap(), (10_000, 5_000));
    assert_eq!(calls, 0, "valid TTLs must not allocate diagnostic labels");
    assert!(validate_quota_ttls(Duration::from_secs(5), Duration::from_secs(10)).is_err());
}

fn reference_quota_keys(prefix: &str, identity: &[u8], window: u64, request: &[u8]) -> CumulativeQuotaKeys {
    let identity = hex::encode(Sha256::digest(identity));
    let request = hex::encode(Sha256::digest(request));
    CumulativeQuotaKeys {
        bucket: format!("{prefix}{{{identity}}}:window:{window}"),
        marker: format!("{prefix}{{{identity}}}:request:{request}"),
    }
}

fn reference_credential_keys(
    prefix: &str,
    namespace: &[u8],
    uid: &[u8],
    alias: &[u8],
    window: u64,
    request: &[u8],
) -> CredentialReservationKeys {
    let mut identity = Sha256::new();
    for part in [namespace, uid, alias] {
        identity.update((part.len() as u64).to_be_bytes());
        identity.update(part);
    }
    let identity = hex::encode(identity.finalize());
    let request = hex::encode(Sha256::digest(request));
    CredentialReservationKeys {
        bucket: format!("{prefix}{{{identity}}}:window:{window}"),
        marker: format!("{prefix}{{{identity}}}:window:{window}:request:{request}"),
    }
}

#[test]
fn opaque_key_builders_preserve_complete_wire_bytes() {
    let long_prefix = "p".repeat(128);
    let binary: Vec<u8> = (0..=u8::MAX).collect();
    let maximum_identity = [b'x'; 512];
    for prefix in ["q:", long_prefix.as_str()] {
        for identity in [b"a".as_slice(), binary.as_slice(), maximum_identity.as_slice()] {
            for window in [0, 9, 10, u64::MAX] {
                assert_eq!(
                    build_cumulative_quota_keys(prefix, identity, window, &binary).unwrap(),
                    reference_quota_keys(prefix, identity, window, &binary)
                );
                assert_eq!(
                    build_credential_reservation_keys(prefix, identity, b"bc", b"d", window, &binary).unwrap(),
                    reference_credential_keys(prefix, identity, b"bc", b"d", window, &binary)
                );
            }
        }
    }
    let first = build_credential_reservation_keys("q:", b"a", b"bc", b"d", 1, b"r").unwrap();
    let second = build_credential_reservation_keys("q:", b"ab", b"c", b"d", 1, b"r").unwrap();
    assert_ne!(
        first.bucket, second.bucket,
        "identity component boundaries remain significant"
    );
    assert_eq!(second, reference_credential_keys("q:", b"ab", b"c", b"d", 1, b"r"));
}

#[test]
fn sha256_hex_uses_only_caller_storage() {
    for byte in 0..=u8::MAX {
        let digest = [byte; 32];
        let mut output = [0u8; 64];
        let (encoded, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| sha256_hex_into(digest, &mut output));
        assert_eq!(encoded, hex::encode(digest));
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn opaque_key_builders_avoid_temporary_digest_allocations() {
    let long_prefix = "p".repeat(128);
    for prefix in ["q:", long_prefix.as_str()] {
        for window in [0, 9, 10, u64::MAX] {
            let (expected, old) = crate::test_allocation_counter::count_allocation_stats(|| {
                reference_quota_keys(prefix, b"identity", window, b"request")
            });
            let (actual, new) = crate::test_allocation_counter::count_allocation_stats(|| {
                build_cumulative_quota_keys(prefix, b"identity", window, b"request")
            });
            assert_eq!(actual.unwrap(), expected);
            assert_eq!(new.calls + 2, old.calls);
            assert_eq!(new.requested_bytes + 128, old.requested_bytes);

            let (expected, old) = crate::test_allocation_counter::count_allocation_stats(|| {
                reference_credential_keys(prefix, b"ns", b"uid", b"alias", window, b"request")
            });
            let (actual, new) = crate::test_allocation_counter::count_allocation_stats(|| {
                build_credential_reservation_keys(prefix, b"ns", b"uid", b"alias", window, b"request")
            });
            assert_eq!(actual.unwrap(), expected);
            assert_eq!(new.calls + 2, old.calls);
            assert_eq!(new.requested_bytes + 128, old.requested_bytes);
        }
    }
}

#[test]
fn credential_keys_are_opaque_scoped_and_same_slot() {
    let keys = build_credential_reservation_keys(
        "edgion:credential:",
        b"tenant-ns",
        b"service-uid-sensitive",
        b"primary-key",
        42,
        b"attempt-request-id",
    )
    .unwrap();
    assert!(!keys.bucket.contains("service-uid-sensitive"));
    assert!(!keys.bucket.contains("primary-key"));
    assert!(!keys.marker.contains("attempt-request-id"));
    assert!(keys.bucket.ends_with(":window:42"));
    assert!(validate_same_slot_key_pair(&keys.bucket, &keys.marker, "credential reservation").is_ok());

    let other_scope = build_credential_reservation_keys(
        "edgion:credential:",
        b"other-ns",
        b"service-uid-sensitive",
        b"primary-key",
        42,
        b"attempt-request-id",
    )
    .unwrap();
    assert_ne!(keys.bucket, other_scope.bucket);
    assert!(build_credential_reservation_keys("bad{", b"ns", b"uid", b"alias", 1, b"id").is_err());
    assert!(build_credential_reservation_keys("ok:", b"", b"uid", b"alias", 1, b"id").is_err());
}

#[test]
fn credential_reserve_is_atomic_bounded_and_idempotent() {
    let limits = CredentialReservationLimits {
        rpm: Some(2),
        tpm: Some(100),
    };
    let mut ledger = MemoryCredentialLedger::default();
    let reserve = CredentialReservationOperation::Reserve {
        estimated_input_tokens: 20,
        max_output_tokens: 30,
    };
    let first = ledger.apply("request-a", limits, reserve);
    assert_eq!(first.status, CredentialReservationStatus::Reserved);
    assert_eq!((first.rpm_used, first.tpm_used), (1, 50));
    let duplicate = ledger.apply("request-a", limits, reserve);
    assert_eq!(duplicate.status, CredentialReservationStatus::Duplicate);
    assert_eq!((duplicate.rpm_used, duplicate.tpm_used), (1, 50));

    let denied_tpm = ledger.apply(
        "request-b",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 1,
            max_output_tokens: 50,
        },
    );
    assert_eq!(denied_tpm.status, CredentialReservationStatus::DeniedTpm);
    assert_eq!((denied_tpm.rpm_used, denied_tpm.tpm_used), (1, 50));

    let allowed = ledger.apply(
        "request-b",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 10,
            max_output_tokens: 10,
        },
    );
    assert_eq!(allowed.status, CredentialReservationStatus::Reserved);
    let denied_rpm = ledger.apply("request-c", limits, reserve);
    assert_eq!(denied_rpm.status, CredentialReservationStatus::DeniedRpm);
    assert_eq!((denied_rpm.rpm_used, denied_rpm.tpm_used), (2, 70));
}

#[test]
fn credential_terminal_reconcile_is_force_charge_and_idempotent() {
    let limits = CredentialReservationLimits {
        rpm: Some(10),
        tpm: Some(100),
    };
    let mut ledger = MemoryCredentialLedger::default();
    ledger.apply(
        "request",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 20,
            max_output_tokens: 30,
        },
    );
    let lower = ledger.apply(
        "request",
        limits,
        CredentialReservationOperation::Reconcile { charged_tokens: 25 },
    );
    assert_eq!(lower.status, CredentialReservationStatus::Reconciled);
    assert_eq!((lower.rpm_used, lower.tpm_used, lower.applied_tpm_delta), (1, 25, -25));
    assert_eq!(
        ledger
            .apply(
                "request",
                limits,
                CredentialReservationOperation::Reconcile { charged_tokens: 25 }
            )
            .status,
        CredentialReservationStatus::Duplicate
    );
    assert_eq!(
        ledger
            .apply(
                "request",
                limits,
                CredentialReservationOperation::Reconcile { charged_tokens: 26 }
            )
            .status,
        CredentialReservationStatus::Conflict
    );

    // A terminal authoritative charge is never dropped merely because it
    // exceeds the configured TPM limit; subsequent attempts are denied.
    let mut over = MemoryCredentialLedger::default();
    over.apply(
        "over",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 10,
            max_output_tokens: 10,
        },
    );
    let charged = over.apply(
        "over",
        limits,
        CredentialReservationOperation::Reconcile { charged_tokens: 150 },
    );
    assert_eq!(charged.tpm_used, 150);
    assert_eq!(charged.tpm_remaining, Some(0));
}

#[test]
fn credential_predispatch_release_returns_both_dimensions() {
    let limits = CredentialReservationLimits {
        rpm: Some(5),
        tpm: Some(500),
    };
    let mut ledger = MemoryCredentialLedger::default();
    ledger.apply(
        "request",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 40,
            max_output_tokens: 60,
        },
    );
    let released = ledger.apply("request", limits, CredentialReservationOperation::ReleasePredispatch);
    assert_eq!(released.status, CredentialReservationStatus::Released);
    assert_eq!(
        (released.rpm_used, released.tpm_used, released.applied_tpm_delta),
        (0, 0, -100)
    );
    assert_eq!(
        ledger
            .apply("request", limits, CredentialReservationOperation::ReleasePredispatch)
            .status,
        CredentialReservationStatus::Duplicate
    );
}

#[test]
fn credential_dimensions_can_be_enabled_independently() {
    let mut rpm_only = MemoryCredentialLedger::default();
    let rpm_limits = CredentialReservationLimits {
        rpm: Some(1),
        tpm: None,
    };
    let result = rpm_only.apply(
        "rpm",
        rpm_limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: u64::MAX,
            max_output_tokens: 1,
        },
    );
    assert_eq!((result.rpm_used, result.tpm_used), (1, 0));
    assert_eq!((result.rpm_remaining, result.tpm_remaining), (Some(0), None));

    let mut tpm_only = MemoryCredentialLedger::default();
    let tpm_limits = CredentialReservationLimits {
        rpm: None,
        tpm: Some(50),
    };
    let result = tpm_only.apply(
        "tpm",
        tpm_limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 20,
            max_output_tokens: 30,
        },
    );
    assert_eq!((result.rpm_used, result.tpm_used), (0, 50));
    assert_eq!((result.rpm_remaining, result.tpm_remaining), (None, Some(0)));
}

#[test]
fn dispatched_ambiguous_outcome_retains_full_reservation() {
    let limits = CredentialReservationLimits {
        rpm: Some(5),
        tpm: Some(500),
    };
    let mut ledger = MemoryCredentialLedger::default();
    let reserved = ledger.apply(
        "request",
        limits,
        CredentialReservationOperation::Reserve {
            estimated_input_tokens: 40,
            max_output_tokens: 60,
        },
    );
    // Ambiguous dispatched outcome intentionally performs no settlement.
    assert_eq!((reserved.rpm_used, reserved.tpm_used), (1, 100));
    assert_eq!((ledger.rpm_used, ledger.tpm_used), (1, 100));
    assert_eq!(
        ledger.markers.get("request"),
        Some(&MemoryMarker::Reserved { rpm: 1, tpm: 100 })
    );
}

#[test]
fn credential_result_parser_and_input_bounds_are_strict() {
    let limits = CredentialReservationLimits {
        rpm: Some(10),
        tpm: Some(100),
    };
    let parsed = CredentialReservationResult::from_lua_result(&[0, 1, 30, 9, 70, 30, 30], limits).unwrap();
    assert_eq!(parsed.status, CredentialReservationStatus::Reserved);
    assert!(CredentialReservationResult::from_lua_result(&[0, 1, 30, 9, 70, 30], limits).is_err());
    assert!(CredentialReservationResult::from_lua_result(&[7, 1, 30, 9, 70, 30, 30], limits).is_err());
    assert!(CredentialReservationResult::from_lua_result(&[0, 1, 30, 8, 70, 30, 30], limits).is_err());
    assert!(CredentialReservationResult::from_lua_result(&[3, 1, 30, 9, 70, 30, 1], limits).is_err());
    assert!(CredentialReservationOperation::Reserve {
        estimated_input_tokens: u64::MAX,
        max_output_tokens: 1,
    }
    .amount()
    .is_err());
    assert!(validate_credential_reservation_ttl(Duration::ZERO).is_err());
    assert!(validate_credential_reservation_ttl(Duration::from_millis(MAX_SAFE_LUA_INTEGER + 1)).is_err());
}

#[test]
fn credential_lua_uses_one_atomic_bounded_ledger() {
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("redis.call('HMGET', bucket_key, 'rpm', 'tpm')"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("state', 'reserved'"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("state', 'reconciled'"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("state', 'released'"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("max_safe = 9007199254740991"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("redis.call('PEXPIRE', bucket_key, ttl_ms)"));
    assert!(CREDENTIAL_RESERVATION_SCRIPT.contains("redis.call('PEXPIRE', marker_key, ttl_ms)"));
    assert!(!CREDENTIAL_RESERVATION_SCRIPT.contains("math.random"));
}

#[test]
fn fixed_window_key_bytes_cover_numeric_and_opaque_boundaries() {
    let long_key = "key:".repeat(1024);
    for (prefix, key) in [
        ("", ""),
        ("p:", "ip:1:2"),
        ("{scope}:", "é:€"),
        ("long:", long_key.as_str()),
    ] {
        for index in [0, 1, usize::MAX] {
            for window in [0_i64, 9, 10, 99, 100, (MAX_SAFE_LUA_INTEGER / 100) as i64, i64::MAX] {
                let suffix = window.to_string();
                let expected = format!("{prefix}fw:{index}:{key}:{window}");
                assert_eq!(build_redis_key(prefix, "fw", index, key, &suffix), expected);
                assert_eq!(build_redis_key_with_suffix(prefix, "fw", index, key, window), expected);
            }
            assert_eq!(
                build_redis_key(prefix, "fw", index, key, ""),
                format!("{prefix}fw:{index}:{key}")
            );
        }
    }
}

#[test]
fn fixed_window_key_avoids_temporary_numeric_string_allocation() {
    use std::hint::black_box;
    let long_key = "key:".repeat(1024);
    for (prefix, key) in [
        ("", ""),
        ("p:", "ip:1:2"),
        ("{scope}:", "é:€"),
        ("long:", long_key.as_str()),
    ] {
        for index in [0, 1, usize::MAX] {
            for window in [0_i64, 9, 10, 99, 100, (MAX_SAFE_LUA_INTEGER / 100) as i64, i64::MAX] {
                let (old, old_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                    let suffix = black_box(window).to_string();
                    build_redis_key(
                        black_box(prefix),
                        black_box("fw"),
                        black_box(index),
                        black_box(key),
                        &suffix,
                    )
                });
                let (new, new_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                    build_redis_key_with_suffix(
                        black_box(prefix),
                        black_box("fw"),
                        black_box(index),
                        black_box(key),
                        black_box(window),
                    )
                });
                assert_eq!(new, old);
                assert_eq!(new_stats.calls + 1, old_stats.calls, "{prefix:?}/{index}/{window}");
                assert!(new_stats.requested_bytes < old_stats.requested_bytes);
            }
        }
    }
}
