//! Shared Redis rate-limit execution core.
//!
//! Extracted from the HTTP `rate_limit_redis` plugin so both the HTTP request
//! limiter and the L4 connection-rate limiter share one implementation of the
//! shared script descriptors, EVALSHA-with-EVAL-fallback machinery and Lua algorithms.
//! This core is protocol-agnostic: it never touches `PluginSession` /
//! `PluginLog` / HTTP responses. Callers map `RateLimitResult.allowed` to their
//! own allow/deny action.

pub mod scripts;

use crate::link_sys::providers::redis::RedisLinkClient;
use edgion_resources::redis_rate_limit::{
    validate_rate_limit_call, validate_rate_limit_policy, validate_rate_limit_units,
};
use edgion_resources::resources::edgion_plugins::{RateLimitAlgorithm, RateLimitPolicy};
use fred::{
    interfaces::LuaInterface,
    types::{MultipleKeys, Value},
};
use scripts::{
    CREDENTIAL_RESERVATION_SCRIPT, CUMULATIVE_QUOTA_SCRIPT, FIXED_WINDOW_SCRIPT, SLIDING_WINDOW_SCRIPT,
    TOKEN_BUCKET_SCRIPT,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::fmt::Display;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Largest integer represented exactly by the Redis Lua 5.1 number type.
pub use edgion_resources::redis_rate_limit::MAX_SAFE_LUA_INTEGER;

/// Opaque, same-slot Redis keys for one cumulative quota operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CumulativeQuotaKeys {
    pub bucket: String,
    pub marker: String,
}

fn validate_opaque_prefix(prefix: &str, mechanism: &str) -> anyhow::Result<()> {
    if prefix.is_empty()
        || prefix.len() > 128
        || prefix.contains('{')
        || prefix.contains('}')
        || !prefix.bytes().all(|byte| byte.is_ascii_graphic())
    {
        anyhow::bail!("{mechanism} key prefix must be 1..=128 printable ASCII bytes without braces");
    }
    Ok(())
}

/// Opaque same-slot keys for one credential/window reservation ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialReservationKeys {
    pub bucket: String,
    pub marker: String,
}

/// Build a credential bucket from namespace scope, immutable AI-service UID,
/// credential alias, and frozen window ID. All raw identities and the internal
/// request ID are digested before entering Redis keyspace.
pub fn build_credential_reservation_keys(
    prefix: &str,
    namespace_scope: &[u8],
    ai_backend_uid: &[u8],
    credential_alias: &[u8],
    window_id: u64,
    internal_request_id: &[u8],
) -> anyhow::Result<CredentialReservationKeys> {
    validate_opaque_prefix(prefix, "credential reservation")?;
    for (name, value, max) in [
        ("namespace scope", namespace_scope, 512usize),
        ("AI backend UID", ai_backend_uid, 512usize),
        ("credential alias", credential_alias, 512usize),
        ("internal request ID", internal_request_id, 256usize),
    ] {
        if value.is_empty() || value.len() > max {
            anyhow::bail!("credential reservation {name} must be 1..={max} bytes");
        }
    }
    let mut identity = Sha256::new();
    for value in [namespace_scope, ai_backend_uid, credential_alias] {
        identity.update((value.len() as u64).to_be_bytes());
        identity.update(value);
    }
    let identity_digest = hex::encode(identity.finalize());
    let request_digest = hex::encode(Sha256::digest(internal_request_id));
    Ok(CredentialReservationKeys {
        bucket: format!("{prefix}{{{identity_digest}}}:window:{window_id}"),
        marker: format!("{prefix}{{{identity_digest}}}:window:{window_id}:request:{request_digest}"),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialReservationLimits {
    pub rpm: Option<u64>,
    pub tpm: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialReservationOperation {
    /// Atomically reserve RPM=1 and TPM=input estimate+maximum output.
    Reserve {
        estimated_input_tokens: u64,
        max_output_tokens: u64,
    },
    /// Final dispatched outcome: replace the conservative TPM reservation with
    /// authoritative usage, or conservative input when usage is unavailable.
    Reconcile { charged_tokens: u64 },
    /// Attempt ended before dispatch, so both RPM and TPM are returned.
    ReleasePredispatch,
}

impl CredentialReservationOperation {
    const fn lua_code(self) -> i64 {
        match self {
            Self::Reserve { .. } => 1,
            Self::Reconcile { .. } => 2,
            Self::ReleasePredispatch => 3,
        }
    }

    fn amount(self) -> anyhow::Result<u64> {
        match self {
            Self::Reserve {
                estimated_input_tokens,
                max_output_tokens,
            } => estimated_input_tokens
                .checked_add(max_output_tokens)
                .ok_or_else(|| anyhow::anyhow!("credential reservation token estimate overflow")),
            Self::Reconcile { charged_tokens } => Ok(charged_tokens),
            Self::ReleasePredispatch => Ok(0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialReservationStatus {
    Reserved,
    DeniedRpm,
    DeniedTpm,
    Duplicate,
    Reconciled,
    Released,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialReservationResult {
    pub status: CredentialReservationStatus,
    pub rpm_used: u64,
    pub tpm_used: u64,
    pub rpm_remaining: Option<u64>,
    pub tpm_remaining: Option<u64>,
    pub charged_tpm: u64,
    pub applied_tpm_delta: i64,
}

impl CredentialReservationResult {
    fn from_lua_result(values: &[i64], limits: CredentialReservationLimits) -> anyhow::Result<Self> {
        if values.len() != 7 {
            anyhow::bail!(
                "credential reservation script returned {} fields; expected exactly 7",
                values.len()
            );
        }
        let status = match values[0] {
            0 => CredentialReservationStatus::Reserved,
            1 => CredentialReservationStatus::DeniedRpm,
            2 => CredentialReservationStatus::DeniedTpm,
            3 => CredentialReservationStatus::Duplicate,
            4 => CredentialReservationStatus::Reconciled,
            5 => CredentialReservationStatus::Released,
            6 => CredentialReservationStatus::Conflict,
            code => anyhow::bail!("credential reservation script returned unknown status {code}"),
        };
        let nonnegative = |value: i64, field: &str| -> anyhow::Result<u64> {
            let parsed = u64::try_from(value)
                .map_err(|_| anyhow::anyhow!("credential reservation script returned negative {field}"))?;
            validate_safe_lua_integer(field, parsed)?;
            Ok(parsed)
        };
        let rpm_used = nonnegative(values[1], "RPM used")?;
        let tpm_used = nonnegative(values[2], "TPM used")?;
        let rpm_remaining = nonnegative(values[3], "RPM remaining")?;
        let tpm_remaining = nonnegative(values[4], "TPM remaining")?;
        let charged_tpm = nonnegative(values[5], "charged TPM")?;
        let applied_tpm_delta = values[6];
        if applied_tpm_delta.unsigned_abs() > MAX_SAFE_LUA_INTEGER {
            anyhow::bail!("credential reservation script returned unsafe TPM delta");
        }
        if limits
            .rpm
            .is_some_and(|limit| rpm_remaining != limit.saturating_sub(rpm_used))
            || limits
                .tpm
                .is_some_and(|limit| tpm_remaining != limit.saturating_sub(tpm_used))
        {
            anyhow::bail!("credential reservation script returned inconsistent remaining capacity");
        }
        if !matches!(
            status,
            CredentialReservationStatus::Reserved
                | CredentialReservationStatus::Reconciled
                | CredentialReservationStatus::Released
        ) && applied_tpm_delta != 0
        {
            anyhow::bail!("credential reservation script returned a delta for a non-mutating status");
        }
        Ok(Self {
            status,
            rpm_used,
            tpm_used,
            rpm_remaining: limits.rpm.map(|_| rpm_remaining),
            tpm_remaining: limits.tpm.map(|_| tpm_remaining),
            charged_tpm,
            applied_tpm_delta,
        })
    }
}

/// Build bounded quota keys without exposing a consumer identity or internal
/// request ID to Redis keyspace inspection.
///
/// The complete quota dimension (policy plus consumer/model/provider values)
/// belongs in `quota_identity`. `window_id` is the request-start window frozen
/// by the caller. Both identities are SHA-256 digested; the quota digest is the
/// shared Redis Cluster hash tag.
pub fn build_cumulative_quota_keys(
    prefix: &str,
    quota_identity: &[u8],
    window_id: u64,
    internal_request_id: &[u8],
) -> anyhow::Result<CumulativeQuotaKeys> {
    validate_opaque_prefix(prefix, "cumulative quota")?;
    if quota_identity.is_empty() {
        anyhow::bail!("cumulative quota identity cannot be empty");
    }
    if internal_request_id.is_empty() {
        anyhow::bail!("cumulative quota internal request ID cannot be empty");
    }
    let quota_digest = hex::encode(Sha256::digest(quota_identity));
    let request_digest = hex::encode(Sha256::digest(internal_request_id));
    Ok(CumulativeQuotaKeys {
        bucket: format!("{prefix}{{{quota_digest}}}:window:{window_id}"),
        marker: format!("{prefix}{{{quota_digest}}}:request:{request_digest}"),
    })
}

/// Mutation policy for a cumulative quota update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CumulativeQuotaMode {
    /// Apply only when the resulting aggregate does not exceed the limit.
    /// Suitable for an optional reservation before starting external work.
    Conditional,
    /// Always charge the new cumulative target, including past the limit.
    /// Suitable for generated usage which must never disappear from enforcement.
    Force,
}

impl CumulativeQuotaMode {
    const fn lua_code(self) -> i64 {
        match self {
            Self::Conditional => 1,
            Self::Force => 2,
        }
    }
}

/// Outcome returned by the cumulative quota script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CumulativeQuotaStatus {
    /// A read-only admission check found capacity.
    Admitted,
    /// Admission or a conditional reservation would exceed the limit.
    Denied,
    /// The request's cumulative target was applied.
    Applied,
    /// The exact update was already applied; this retry changed nothing.
    Duplicate,
    /// A newer revision was already applied; this delayed update changed nothing.
    Stale,
    /// The revision or finalized state conflicts with the stored request state.
    Conflict,
}

/// Strictly parsed cumulative quota result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CumulativeQuotaResult {
    pub status: CumulativeQuotaStatus,
    /// Signed change made to the aggregate by this call.
    pub applied_delta: i64,
    pub used: u64,
    pub remaining: u64,
    /// Stored request revision, or zero for a read-only admission result.
    pub revision: u64,
}

impl CumulativeQuotaResult {
    fn from_lua_result(values: &[i64], limit: u64) -> anyhow::Result<Self> {
        if values.len() != 5 {
            anyhow::bail!(
                "cumulative quota script returned {} fields; expected exactly 5",
                values.len()
            );
        }
        let status = match values[0] {
            0 => CumulativeQuotaStatus::Admitted,
            1 => CumulativeQuotaStatus::Denied,
            2 => CumulativeQuotaStatus::Applied,
            3 => CumulativeQuotaStatus::Duplicate,
            4 => CumulativeQuotaStatus::Stale,
            5 => CumulativeQuotaStatus::Conflict,
            code => anyhow::bail!("cumulative quota script returned unknown status {code}"),
        };
        let applied_delta = values[1];
        if applied_delta.unsigned_abs() > MAX_SAFE_LUA_INTEGER {
            anyhow::bail!("cumulative quota script returned an unsafe delta");
        }
        let parse_nonnegative = |value: i64, field: &str| -> anyhow::Result<u64> {
            let value = u64::try_from(value)
                .map_err(|_| anyhow::anyhow!("cumulative quota script returned negative {field}"))?;
            if value > MAX_SAFE_LUA_INTEGER {
                anyhow::bail!("cumulative quota script returned unsafe {field}");
            }
            Ok(value)
        };
        let used = parse_nonnegative(values[2], "used")?;
        let remaining = parse_nonnegative(values[3], "remaining")?;
        let revision = parse_nonnegative(values[4], "revision")?;
        let expected_remaining = limit.saturating_sub(used);
        if remaining != expected_remaining {
            anyhow::bail!(
                "cumulative quota script returned inconsistent remaining value {remaining}; expected {expected_remaining}"
            );
        }
        if status != CumulativeQuotaStatus::Applied && applied_delta != 0 {
            anyhow::bail!("cumulative quota script returned a delta for a non-applied result");
        }
        Ok(Self {
            status,
            applied_delta,
            used,
            remaining,
            revision,
        })
    }
}

/// Parsed result of a single Lua rate-limit reply `{allowed, current, remaining, retry_ms}`.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub current: i64,
    pub remaining: i64,
    pub limit: u64,
    pub retry_ms: i64,
}

fn validate_rate_limit_execution(rate: u64, window_ms: i64, now_ms: i64, cost: u32) -> anyhow::Result<()> {
    let window_ms_u64 =
        u64::try_from(window_ms).map_err(|_| anyhow::anyhow!("rate-limit interval milliseconds are invalid"))?;
    let validated_window_ms = validate_rate_limit_policy(rate, Duration::from_millis(window_ms_u64))
        .map_err(|error| anyhow::anyhow!("invalid rate-limit policy: {error}"))?;
    if validated_window_ms != window_ms {
        anyhow::bail!("rate-limit interval milliseconds are invalid");
    }
    validate_rate_limit_call(i64::from(cost), now_ms)
        .map_err(|error| anyhow::anyhow!("invalid rate-limit call: {error}"))?;
    Ok(())
}

fn validate_rate_limit_batch(policies: &[RateLimitPolicy], cost: u32, now_ms: i64) -> anyhow::Result<()> {
    if policies.is_empty() {
        anyhow::bail!("redis_rate_limiter: no policy after iteration — invariant violated");
    }
    validate_rate_limit_call(i64::from(cost), now_ms)
        .map_err(|error| anyhow::anyhow!("invalid rate-limit call: {error}"))?;
    for policy in policies {
        let interval = policy
            .interval_duration
            .ok_or_else(|| anyhow::anyhow!("rate-limit policy interval was not compiled"))?;
        validate_rate_limit_policy(policy.rate, interval)
            .map_err(|error| anyhow::anyhow!("invalid rate-limit policy: {error}"))?;
    }
    Ok(())
}

impl RateLimitResult {
    pub fn from_lua_result(values: &[i64], limit: u64) -> anyhow::Result<Self> {
        validate_rate_limit_units(limit)
            .map_err(|error| anyhow::anyhow!("invalid rate-limit result limit: {error}"))?;
        if values.len() != 4 {
            anyhow::bail!("rate-limit script returned {} fields; expected exactly 4", values.len());
        }
        let allowed = match values[0] {
            0 => false,
            1 => true,
            code => anyhow::bail!("rate-limit script returned invalid allow status {code}"),
        };
        let bounded = |value: i64, field: &str| -> anyhow::Result<i64> {
            let unsigned =
                u64::try_from(value).map_err(|_| anyhow::anyhow!("rate-limit script returned negative {field}"))?;
            validate_safe_lua_integer(field, unsigned)?;
            Ok(value)
        };
        let current = bounded(values[1], "current")?;
        let remaining = bounded(values[2], "remaining")?;
        let retry_ms = bounded(values[3], "retry milliseconds")?;
        if u64::try_from(remaining).unwrap_or(u64::MAX) > limit {
            anyhow::bail!("rate-limit script returned remaining capacity above the configured limit");
        }
        if allowed {
            if u64::try_from(current).unwrap_or(u64::MAX) > limit || retry_ms != 0 {
                anyhow::bail!("rate-limit script returned an inconsistent allow result");
            }
        } else if remaining != 0 {
            anyhow::bail!("rate-limit script returned an inconsistent deny result");
        }
        Ok(Self {
            allowed,
            current,
            remaining,
            limit,
            retry_ms,
        })
    }

    /// Aggregate across policies: deny wins; among allows keep the tightest remaining.
    pub fn merge(existing: Option<Self>, new: Self) -> Self {
        match existing {
            None => new,
            Some(prev) => {
                if !prev.allowed || !new.allowed {
                    RateLimitResult {
                        allowed: false,
                        current: prev.current.max(new.current),
                        remaining: 0,
                        limit: prev.limit.min(new.limit),
                        retry_ms: prev.retry_ms.max(new.retry_ms),
                    }
                } else if new.remaining < prev.remaining {
                    new
                } else {
                    prev
                }
            }
        }
    }
}

/// Build a single Redis key: `{prefix}{algo}:{policy_idx}:{limit_key}[:{suffix}]`.
pub fn build_redis_key(prefix: &str, algo: &str, policy_idx: usize, limit_key: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}")
    } else {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}:{suffix}")
    }
}

/// Truncate the variable key segment on a UTF-8 boundary.
pub fn truncate_limit_key(raw: &str, max_len: usize) -> &str {
    if raw.len() > max_len {
        let mut end = max_len;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        &raw[..end]
    } else {
        raw
    }
}

struct ScriptDescriptor {
    source: &'static str,
    sha1: String,
}

impl ScriptDescriptor {
    fn new(source: &'static str) -> Self {
        Self {
            source,
            // Redis addresses cached scripts by SHA1; this is not a security digest.
            sha1: format!("{:x}", Sha1::digest(source.as_bytes())),
        }
    }
}

struct ScriptDescriptors {
    sliding_window: ScriptDescriptor,
    fixed_window: ScriptDescriptor,
    token_bucket: ScriptDescriptor,
    cumulative_quota: ScriptDescriptor,
    credential_reservation: ScriptDescriptor,
}

static SCRIPTS: LazyLock<ScriptDescriptors> = LazyLock::new(|| ScriptDescriptors {
    sliding_window: ScriptDescriptor::new(SLIDING_WINDOW_SCRIPT),
    fixed_window: ScriptDescriptor::new(FIXED_WINDOW_SCRIPT),
    token_bucket: ScriptDescriptor::new(TOKEN_BUCKET_SCRIPT),
    cumulative_quota: ScriptDescriptor::new(CUMULATIVE_QUOTA_SCRIPT),
    credential_reservation: ScriptDescriptor::new(CREDENTIAL_RESERVATION_SCRIPT),
});

// Fred preserves the Redis error code in the first token of an Unknown error.
// Only a cache miss proves the script did not run; other errors must not replay it.
fn is_missing_script(error: &fred::error::Error) -> bool {
    error.kind() == &fred::error::ErrorKind::Unknown
        && error.details().split_ascii_whitespace().next() == Some("NOSCRIPT")
}

/// References process-shared script descriptors; remote cache misses are handled per invocation.
pub struct RedisRateLimiter {
    scripts: &'static ScriptDescriptors,
}

impl Default for RedisRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RedisRateLimiter {
    pub fn new() -> Self {
        Self {
            // Initialize the fixed descriptor set at construction, not on first admission.
            scripts: LazyLock::force(&SCRIPTS),
        }
    }

    async fn eval_script(
        &self,
        client: &RedisLinkClient,
        script: &ScriptDescriptor,
        keys: Vec<String>,
        args: Vec<String>,
    ) -> anyhow::Result<Vec<i64>> {
        let keys = MultipleKeys::from(keys);
        let args = Value::Array(args.into_iter().map(Value::from).collect());
        match client
            .pool()
            .evalsha::<Vec<i64>, _, _, _>(script.sha1.as_str(), keys.clone(), args.clone())
            .await
        {
            Ok(result) => Ok(result),
            Err(ref e) if is_missing_script(e) => client
                .pool()
                .eval(script.source, keys, args)
                .await
                .map_err(|e| anyhow::anyhow!("Redis EVAL fallback failed: {:?}", e)),
            Err(e) => Err(anyhow::anyhow!("Redis EVALSHA failed: {:?}", e)),
        }
    }

    /// Read the aggregate for a caller-selected quota window and decide whether
    /// another unit may start. This operation does not create or extend the key.
    pub async fn cumulative_quota_admission(
        &self,
        client: &RedisLinkClient,
        quota_key: &str,
        limit: u64,
    ) -> anyhow::Result<CumulativeQuotaResult> {
        validate_quota_common(client, quota_key, limit)?;
        let values = self
            .eval_script(
                client,
                &self.scripts.cumulative_quota,
                // Admission reads only KEYS[1]. Reusing it as KEYS[2] keeps the
                // invocation valid in Redis Cluster without inventing a marker.
                vec![quota_key.to_string(), quota_key.to_string()],
                vec![
                    "0".to_string(),
                    limit.to_string(),
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                ],
            )
            .await?;
        CumulativeQuotaResult::from_lua_result(&values, limit)
    }

    /// Atomically move one logical request to a new cumulative charged target.
    ///
    /// `bucket_key` includes the request-start window chosen by the caller;
    /// `marker_key` is derived from the internal request ID. Both keys must be
    /// opaque (prefer [`build_cumulative_quota_keys`]) and use the same non-empty
    /// Redis Cluster hash tag. `revision` increases for every distinct update. Exact
    /// retries are no-ops; stale revisions are ignored. A final update may lower
    /// the target, making authoritative reconciliation a signed correction.
    ///
    /// `bucket_ttl` is the remaining window lifetime plus cleanup grace.
    /// `marker_ttl` bounds idempotency and must cover the maximum request
    /// duration plus retry grace. A retry after marker expiry is a new charge.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_cumulative_quota(
        &self,
        client: &RedisLinkClient,
        bucket_key: &str,
        marker_key: &str,
        revision: u64,
        cumulative_target: u64,
        final_update: bool,
        limit: u64,
        bucket_ttl: Duration,
        marker_ttl: Duration,
        mode: CumulativeQuotaMode,
    ) -> anyhow::Result<CumulativeQuotaResult> {
        validate_quota_common(client, bucket_key, limit)?;
        validate_quota_key_pair(bucket_key, marker_key)?;
        validate_safe_lua_integer("revision", revision)?;
        validate_safe_lua_integer("cumulative target", cumulative_target)?;
        let (bucket_ttl_ms, marker_ttl_ms) = validate_quota_ttls(bucket_ttl, marker_ttl)?;

        let values = self
            .eval_script(
                client,
                &self.scripts.cumulative_quota,
                vec![bucket_key.to_string(), marker_key.to_string()],
                vec![
                    mode.lua_code().to_string(),
                    limit.to_string(),
                    bucket_ttl_ms.to_string(),
                    marker_ttl_ms.to_string(),
                    revision.to_string(),
                    cumulative_target.to_string(),
                    i64::from(final_update).to_string(),
                ],
            )
            .await?;
        CumulativeQuotaResult::from_lua_result(&values, limit)
    }

    /// Atomically reserve or settle one credential attempt within a frozen
    /// caller-selected window. A dispatched ambiguous outcome intentionally
    /// calls none of the settlement operations, retaining the conservative
    /// reservation until this TTL expires.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_credential_reservation(
        &self,
        client: &RedisLinkClient,
        keys: &CredentialReservationKeys,
        limits: CredentialReservationLimits,
        ttl: Duration,
        operation: CredentialReservationOperation,
    ) -> anyhow::Result<CredentialReservationResult> {
        if !client.healthy() {
            anyhow::bail!("Redis client '{}' is not healthy", client.name());
        }
        validate_same_slot_key_pair(&keys.bucket, &keys.marker, "credential reservation")?;
        if limits.rpm.is_none() && limits.tpm.is_none() {
            anyhow::bail!("credential reservation requires an RPM or TPM limit");
        }
        for (name, value) in [("RPM limit", limits.rpm), ("TPM limit", limits.tpm)] {
            if let Some(value) = value {
                if value == 0 {
                    anyhow::bail!("credential reservation {name} must be at least 1");
                }
                validate_safe_lua_integer(name, value)?;
            }
        }
        let amount = if limits.tpm.is_some() { operation.amount()? } else { 0 };
        validate_safe_lua_integer("credential reservation token amount", amount)?;
        let ttl_ms = validate_credential_reservation_ttl(ttl)?;

        let values = self
            .eval_script(
                client,
                &self.scripts.credential_reservation,
                vec![keys.bucket.clone(), keys.marker.clone()],
                vec![
                    operation.lua_code().to_string(),
                    limits.rpm.unwrap_or(0).to_string(),
                    limits.tpm.unwrap_or(0).to_string(),
                    ttl_ms.to_string(),
                    amount.to_string(),
                ],
            )
            .await?;
        CredentialReservationResult::from_lua_result(&values, limits)
    }

    /// Evaluate one policy against `limit_key` (already truncated). `now_ms` is
    /// passed explicitly for testability.
    #[allow(clippy::too_many_arguments)]
    pub async fn eval_policy(
        &self,
        client: &RedisLinkClient,
        algorithm: &RateLimitAlgorithm,
        key_prefix: &str,
        policy_idx: usize,
        limit_key: &str,
        rate: u64,
        window_ms: i64,
        now_ms: i64,
        cost: u32,
    ) -> anyhow::Result<RateLimitResult> {
        validate_rate_limit_execution(rate, window_ms, now_ms, cost)?;
        match algorithm {
            RateLimitAlgorithm::SlidingWindow => {
                let key = build_redis_key(key_prefix, "sw", policy_idx, limit_key, "");
                let args = vec![
                    rate.to_string(),
                    window_ms.to_string(),
                    now_ms.to_string(),
                    cost.to_string(),
                ];
                let v = self
                    .eval_script(client, &self.scripts.sliding_window, vec![key], args)
                    .await?;
                RateLimitResult::from_lua_result(&v, rate)
            }
            RateLimitAlgorithm::FixedWindow => {
                let window_id = now_ms / window_ms;
                let key = build_redis_key(key_prefix, "fw", policy_idx, limit_key, &window_id.to_string());
                let args = vec![rate.to_string(), window_ms.to_string(), cost.to_string()];
                let v = self
                    .eval_script(client, &self.scripts.fixed_window, vec![key], args)
                    .await?;
                RateLimitResult::from_lua_result(&v, rate)
            }
            RateLimitAlgorithm::TokenBucket => {
                let key = build_redis_key(key_prefix, "tb", policy_idx, limit_key, "");
                let args = vec![
                    rate.to_string(),
                    rate.to_string(),
                    window_ms.to_string(),
                    now_ms.to_string(),
                    cost.to_string(),
                ];
                let v = self
                    .eval_script(client, &self.scripts.token_bucket, vec![key], args)
                    .await?;
                RateLimitResult::from_lua_result(&v, rate)
            }
        }
    }

    /// Multi-policy orchestration: truncate the key once, compute `now_ms`, run
    /// every policy, return the most restrictive result.
    pub async fn check(
        &self,
        client: &RedisLinkClient,
        policies: &[RateLimitPolicy],
        key_prefix: &str,
        cost: u32,
        max_key_len: usize,
        limit_key: &str,
    ) -> anyhow::Result<RateLimitResult> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let now_ms = i64::try_from(now_ms).map_err(|_| anyhow::anyhow!("rate-limit timestamp overflow"))?;
        validate_rate_limit_batch(policies, cost, now_ms)?;
        if !client.healthy() {
            anyhow::bail!("Redis client '{}' is not healthy", client.name());
        }
        let safe_key = truncate_limit_key(limit_key, max_key_len);

        let mut most_restrictive: Option<RateLimitResult> = None;
        for (idx, policy) in policies.iter().enumerate() {
            let interval = policy
                .interval_duration
                .ok_or_else(|| anyhow::anyhow!("rate-limit policy interval was not compiled"))?;
            let window_ms = validate_rate_limit_policy(policy.rate, interval)
                .map_err(|error| anyhow::anyhow!("invalid rate-limit policy: {error}"))?;
            let result = self
                .eval_policy(
                    client,
                    &policy.algorithm,
                    key_prefix,
                    idx,
                    safe_key,
                    policy.rate,
                    window_ms,
                    now_ms,
                    cost,
                )
                .await?;
            most_restrictive = Some(RateLimitResult::merge(most_restrictive, result));
        }
        most_restrictive
            .ok_or_else(|| anyhow::anyhow!("redis_rate_limiter: no policy after iteration — invariant violated"))
    }
}

fn validate_safe_lua_integer(name: impl Display, value: u64) -> anyhow::Result<()> {
    if value > MAX_SAFE_LUA_INTEGER {
        anyhow::bail!("{name} exceeds the Redis Lua exact-integer limit");
    }
    Ok(())
}

fn validate_quota_ttl(name: &str, ttl: Duration) -> anyhow::Result<u64> {
    let ttl_ms = u64::try_from(ttl.as_millis())
        .map_err(|_| anyhow::anyhow!("cumulative quota {name} TTL exceeds u64 milliseconds"))?;
    if ttl_ms == 0 {
        anyhow::bail!("cumulative quota {name} TTL must be at least 1 millisecond");
    }
    validate_safe_lua_integer(format_args!("{name} TTL milliseconds"), ttl_ms)?;
    Ok(ttl_ms)
}

fn validate_quota_ttls(bucket_ttl: Duration, marker_ttl: Duration) -> anyhow::Result<(u64, u64)> {
    let bucket_ttl_ms = validate_quota_ttl("bucket", bucket_ttl)?;
    let marker_ttl_ms = validate_quota_ttl("marker", marker_ttl)?;
    if bucket_ttl_ms < marker_ttl_ms {
        anyhow::bail!("cumulative quota bucket TTL must be at least the marker TTL");
    }
    Ok((bucket_ttl_ms, marker_ttl_ms))
}

fn redis_cluster_hash_tag(key: &str) -> Option<&str> {
    let start = key.find('{')? + 1;
    let end = key[start..].find('}')? + start;
    (end > start).then_some(&key[start..end])
}

fn validate_quota_key_pair(bucket_key: &str, marker_key: &str) -> anyhow::Result<()> {
    validate_same_slot_key_pair(bucket_key, marker_key, "cumulative quota")
}

fn validate_same_slot_key_pair(bucket_key: &str, marker_key: &str, mechanism: &str) -> anyhow::Result<()> {
    if marker_key.is_empty() {
        anyhow::bail!("{mechanism} marker key cannot be empty");
    }
    if bucket_key == marker_key {
        anyhow::bail!("{mechanism} bucket and marker keys must be distinct");
    }
    let bucket_tag = redis_cluster_hash_tag(bucket_key)
        .ok_or_else(|| anyhow::anyhow!("{mechanism} bucket key requires a non-empty Redis Cluster hash tag"))?;
    let marker_tag = redis_cluster_hash_tag(marker_key)
        .ok_or_else(|| anyhow::anyhow!("{mechanism} marker key requires a non-empty Redis Cluster hash tag"))?;
    if bucket_tag != marker_tag {
        anyhow::bail!("{mechanism} bucket and marker keys must use the same Redis Cluster hash tag");
    }
    Ok(())
}

fn validate_credential_reservation_ttl(ttl: Duration) -> anyhow::Result<u64> {
    let ttl_ms = u64::try_from(ttl.as_millis())
        .map_err(|_| anyhow::anyhow!("credential reservation TTL exceeds u64 milliseconds"))?;
    if ttl_ms == 0 {
        anyhow::bail!("credential reservation TTL must be at least 1 millisecond");
    }
    validate_safe_lua_integer("credential reservation TTL milliseconds", ttl_ms)?;
    Ok(ttl_ms)
}

fn validate_quota_common(client: &RedisLinkClient, quota_key: &str, limit: u64) -> anyhow::Result<()> {
    if !client.healthy() {
        anyhow::bail!("Redis client '{}' is not healthy", client.name());
    }
    if quota_key.is_empty() {
        anyhow::bail!("cumulative quota key cannot be empty");
    }
    validate_safe_lua_integer("limit", limit)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/common/redis_rate_limiter/mod_test.rs"]
mod tests;
