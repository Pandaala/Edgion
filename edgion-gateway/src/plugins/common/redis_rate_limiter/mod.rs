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
use edgion_resources::resources::edgion_plugins::{RateLimitAlgorithm, RateLimitPolicy};
use fred::interfaces::LuaInterface;
use scripts::{
    CREDENTIAL_RESERVATION_SCRIPT, CUMULATIVE_QUOTA_SCRIPT, FIXED_WINDOW_SCRIPT, SLIDING_WINDOW_SCRIPT,
    TOKEN_BUCKET_SCRIPT,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Largest integer represented exactly by the Redis Lua 5.1 number type.
pub const MAX_SAFE_LUA_INTEGER: u64 = (1_u64 << 53) - 1;

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

impl RateLimitResult {
    pub fn from_lua_result(values: &[i64], limit: u64) -> Self {
        Self {
            allowed: values.first().copied().unwrap_or(0) == 1,
            current: values.get(1).copied().unwrap_or(0),
            remaining: values.get(2).copied().unwrap_or(0),
            limit,
            retry_ms: values.get(3).copied().unwrap_or(0),
        }
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
                Ok(RateLimitResult::from_lua_result(&v, rate))
            }
            RateLimitAlgorithm::FixedWindow => {
                let window_id = now_ms / window_ms;
                let key = build_redis_key(key_prefix, "fw", policy_idx, limit_key, &window_id.to_string());
                let args = vec![rate.to_string(), window_ms.to_string(), cost.to_string()];
                let v = self
                    .eval_script(client, &self.scripts.fixed_window, vec![key], args)
                    .await?;
                Ok(RateLimitResult::from_lua_result(&v, rate))
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
                Ok(RateLimitResult::from_lua_result(&v, rate))
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
        if !client.healthy() {
            anyhow::bail!("Redis client '{}' is not healthy", client.name());
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let safe_key = truncate_limit_key(limit_key, max_key_len);

        let mut most_restrictive: Option<RateLimitResult> = None;
        for (idx, policy) in policies.iter().enumerate() {
            let window_ms = policy.interval_duration.unwrap_or(Duration::from_secs(1)).as_millis() as i64;
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

fn validate_safe_lua_integer(name: &str, value: u64) -> anyhow::Result<()> {
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
    validate_safe_lua_integer(&format!("{name} TTL milliseconds"), ttl_ms)?;
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
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        use fred::error::{Error, ErrorKind};

        for details in ["NOSCRIPT", "NOSCRIPT No matching script. Please use EVAL."] {
            assert!(is_missing_script(&Error::new(ErrorKind::Unknown, details)));
        }
        for details in [
            "",
            "NOSCRIPTING not a Redis cache-miss code",
            "ERR script reported NOSCRIPT after changing a key",
            "WRONGTYPE NOSCRIPT",
            "NOPERM NOSCRIPT",
            "noscript No matching script",
        ] {
            assert!(!is_missing_script(&Error::new(ErrorKind::Unknown, details)));
        }
        for kind in [ErrorKind::Timeout, ErrorKind::IO, ErrorKind::Auth, ErrorKind::Canceled] {
            assert!(!is_missing_script(&Error::new(kind, "NOSCRIPT")));
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
        let r = RateLimitResult::from_lua_result(&[1, 7, 3, 0], 10);
        assert!(r.allowed);
        assert_eq!(r.current, 7);
        assert_eq!(r.remaining, 3);
        assert_eq!(r.limit, 10);
    }

    #[test]
    fn merge_denies_when_any_denies() {
        let allow = RateLimitResult::from_lua_result(&[1, 1, 9, 0], 10);
        let deny = RateLimitResult::from_lua_result(&[0, 11, 0, 500], 10);
        let merged = RateLimitResult::merge(Some(allow), deny);
        assert!(!merged.allowed);
        assert_eq!(merged.retry_ms, 500);
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
        assert!(validate_quota_ttl("marker", Duration::from_millis(MAX_SAFE_LUA_INTEGER + 1),).is_err());
        assert!(validate_quota_ttls(Duration::from_secs(10), Duration::from_secs(5)).is_ok());
        assert!(validate_quota_ttls(Duration::from_secs(5), Duration::from_secs(10)).is_err());
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
}
