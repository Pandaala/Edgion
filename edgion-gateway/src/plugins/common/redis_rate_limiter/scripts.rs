//! Redis Lua scripts for atomic rate limiting.
//!
//! All rate limit checks are executed atomically via Redis Lua scripts (EVAL/EVALSHA).
//! Rate-limit scripts operate on one key. The cumulative quota script uses two
//! keys with one required shared hash tag for Redis Cluster compatibility.
//!
//! ## Return Value Convention
//! Request-rate scripts return `{allowed, current, remaining, retry_ms}`. The
//! cumulative quota and credential reservation scripts document their stricter
//! typed result shapes below.
//!
//! ## EVALSHA Optimization
//! Script SHA1 descriptors are shared locally and invoked via EVALSHA.
//! Only NOSCRIPT (Redis restart / SCRIPT FLUSH) triggers fallback to EVAL.

/// Cumulative quota ledger — one bucket HASH and one request-marker HASH.
///
/// The caller freezes the window in `KEYS[1]` and derives `KEYS[2]` from an
/// internal logical request ID. Both keys must carry the same Redis Cluster
/// hash tag so the script remains atomic without CROSSSLOT. Operator or client
/// identity must be digested before it enters either opaque key.
///
/// The bucket stores only `used`, so its size is constant for arbitrarily long
/// windows. The marker stores `revision`, `target`, and `final` and expires on a
/// separately bounded request/retry horizon. A retry after marker expiry is no
/// longer idempotent; callers must size that TTL for the maximum request and
/// retry lifetime.
///
/// KEYS[1] = opaque aggregate bucket key for the caller-selected window
/// KEYS[2] = opaque marker key derived from the internal logical request ID
/// ARGV[1] = operation (`0` admission/read, `1` conditional apply, `2` force apply)
/// ARGV[2] = quota limit
/// ARGV[3] = bucket TTL in milliseconds (window remainder plus grace)
/// ARGV[4] = marker TTL in milliseconds (maximum request/retry lifetime)
/// ARGV[5] = monotonically increasing request revision
/// ARGV[6] = cumulative charged target for this request
/// ARGV[7] = final flag (`0` or `1`)
///
/// Status codes:
/// `0` admitted, `1` denied, `2` applied, `3` duplicate, `4` stale,
/// `5` conflicting revision/final state.
///
/// Returns: {status, signed_applied_delta, used, remaining, stored_revision}
pub const CUMULATIVE_QUOTA_SCRIPT: &str = r#"
local key = KEYS[1]
local marker_key = KEYS[2]
local operation = tonumber(ARGV[1])
local quota_limit = tonumber(ARGV[2])
local bucket_ttl_ms = tonumber(ARGV[3])
local marker_ttl_ms = tonumber(ARGV[4])
local revision = tonumber(ARGV[5])
local target = tonumber(ARGV[6])
local is_final = tonumber(ARGV[7])
local max_safe = 9007199254740991

local function valid_nonnegative_integer(value)
    return value ~= nil and value >= 0 and value <= max_safe and value % 1 == 0
end

if operation == nil or operation < 0 or operation > 2 or operation % 1 ~= 0 then
    return redis.error_reply('invalid cumulative quota operation')
end
if not valid_nonnegative_integer(quota_limit) then
    return redis.error_reply('invalid cumulative quota limit')
end

local used_raw = redis.call('HGET', key, 'used')
local used = 0
if used_raw then
    used = tonumber(used_raw)
    if not valid_nonnegative_integer(used) then
        return redis.error_reply('corrupt cumulative quota aggregate')
    end
end

local function remaining(current)
    return math.max(0, quota_limit - current)
end

if operation == 0 then
    if used >= quota_limit then
        return {1, 0, used, 0, 0}
    end
    return {0, 0, used, remaining(used), 0}
end

if not valid_nonnegative_integer(bucket_ttl_ms) or bucket_ttl_ms == 0
    or not valid_nonnegative_integer(marker_ttl_ms) or marker_ttl_ms == 0 then
    return redis.error_reply('invalid cumulative quota ttl')
end
if not valid_nonnegative_integer(revision) or not valid_nonnegative_integer(target) then
    return redis.error_reply('invalid cumulative quota request state')
end
if is_final ~= 0 and is_final ~= 1 then
    return redis.error_reply('invalid cumulative quota final flag')
end

local previous_revision = 0
local previous_target = 0
local previous_final = 0
local state = redis.call('HMGET', marker_key, 'revision', 'target', 'final')
local state_fields = 0
for index = 1, 3 do
    if state[index] then state_fields = state_fields + 1 end
end
if state_fields ~= 0 and state_fields ~= 3 then
    return redis.error_reply('corrupt cumulative quota request state')
end

local function refresh_expiry()
    redis.call('PEXPIRE', key, bucket_ttl_ms)
    redis.call('PEXPIRE', marker_key, marker_ttl_ms)
end

if state_fields == 3 then
    previous_revision = tonumber(state[1])
    previous_target = tonumber(state[2])
    previous_final = tonumber(state[3])
    if not valid_nonnegative_integer(previous_revision)
        or not valid_nonnegative_integer(previous_target)
        or (previous_final ~= 0 and previous_final ~= 1) then
        return redis.error_reply('corrupt cumulative quota request state')
    end

    if previous_final == 1 then
        refresh_expiry()
        if target == previous_target and is_final == 1 then
            return {3, 0, used, remaining(used), previous_revision}
        end
        return {5, 0, used, remaining(used), previous_revision}
    end
    if revision < previous_revision then
        refresh_expiry()
        return {4, 0, used, remaining(used), previous_revision}
    end
    if revision == previous_revision then
        refresh_expiry()
        if target == previous_target and is_final == previous_final then
            return {3, 0, used, remaining(used), previous_revision}
        end
        return {5, 0, used, remaining(used), previous_revision}
    end
    if is_final == 0 and target < previous_target then
        refresh_expiry()
        return {5, 0, used, remaining(used), previous_revision}
    end
end

local delta = target - previous_target
local new_used = used + delta
if not valid_nonnegative_integer(new_used) then
    return redis.error_reply('cumulative quota aggregate overflow or underflow')
end

if operation == 1 and new_used > quota_limit then
    return {1, 0, used, remaining(used), previous_revision}
end

redis.call('HSET', key, 'used', new_used)
redis.call('HSET', marker_key, 'revision', ARGV[5], 'target', ARGV[6], 'final', ARGV[7])
refresh_expiry()
return {2, delta, new_used, remaining(new_used), revision}
"#;

/// Atomic per-attempt credential RPM/TPM reservation ledger.
///
/// KEYS[1] = opaque credential/window bucket HASH (`rpm`, `tpm`)
/// KEYS[2] = opaque idempotent request marker HASH
/// ARGV[1] = operation: 1 reserve, 2 reconcile terminal usage, 3 predispatch release
/// ARGV[2] = RPM limit
/// ARGV[3] = TPM limit
/// ARGV[4] = TTL milliseconds (window remainder plus bounded grace)
/// ARGV[5] = token amount (reservation or reconciled charge; ignored for release)
///
/// Marker states are `reserved`, `reconciled`, and `released`. A dispatched
/// ambiguous outcome deliberately makes no follow-up call, leaving the full
/// reservation until the bucket/window expires.
///
/// Returns `{status, rpm_used, tpm_used, rpm_remaining, tpm_remaining,
/// charged_tpm, signed_tpm_delta}`. Status: 0 reserved, 1 RPM denied,
/// 2 TPM denied, 3 duplicate, 4 reconciled, 5 released, 6 conflict.
pub const CREDENTIAL_RESERVATION_SCRIPT: &str = r#"
local bucket_key = KEYS[1]
local marker_key = KEYS[2]
local operation = tonumber(ARGV[1])
local rpm_limit = tonumber(ARGV[2])
local tpm_limit = tonumber(ARGV[3])
local ttl_ms = tonumber(ARGV[4])
local amount = tonumber(ARGV[5])
local max_safe = 9007199254740991

local function valid_nonnegative_integer(value)
    return value ~= nil and value >= 0 and value <= max_safe and value % 1 == 0
end
if operation == nil or operation < 1 or operation > 3 or operation % 1 ~= 0 then
    return redis.error_reply('invalid credential reservation operation')
end
if not valid_nonnegative_integer(rpm_limit)
    or not valid_nonnegative_integer(tpm_limit)
    or (rpm_limit == 0 and tpm_limit == 0)
    or not valid_nonnegative_integer(ttl_ms) or ttl_ms == 0
    or not valid_nonnegative_integer(amount) then
    return redis.error_reply('invalid credential reservation input')
end

local bucket = redis.call('HMGET', bucket_key, 'rpm', 'tpm')
local rpm_used = tonumber(bucket[1]) or 0
local tpm_used = tonumber(bucket[2]) or 0
if not valid_nonnegative_integer(rpm_used) or not valid_nonnegative_integer(tpm_used) then
    return redis.error_reply('corrupt credential reservation bucket')
end

local marker = redis.call('HMGET', marker_key, 'state', 'reserved_rpm', 'reserved_tpm', 'charged_tpm')
local marker_fields = 0
for index = 1, 4 do
    if marker[index] then marker_fields = marker_fields + 1 end
end
if marker_fields ~= 0 and marker_fields ~= 4 then
    return redis.error_reply('corrupt credential reservation marker')
end

local function remaining(limit, used)
    return math.max(0, limit - used)
end
local function reply(status, charged, delta)
    return {status, rpm_used, tpm_used, remaining(rpm_limit, rpm_used),
        remaining(tpm_limit, tpm_used), charged, delta}
end
local function refresh_expiry()
    redis.call('PEXPIRE', bucket_key, ttl_ms)
    redis.call('PEXPIRE', marker_key, ttl_ms)
end

if operation == 1 then
    if marker_fields == 4 then
        local charged = tonumber(marker[4])
        if not valid_nonnegative_integer(charged) then
            return redis.error_reply('corrupt credential reservation marker')
        end
        refresh_expiry()
        return reply(3, charged, 0)
    end
    local reserve_rpm = 0
    if rpm_limit > 0 then reserve_rpm = 1 end
    if rpm_limit > 0 and rpm_used >= rpm_limit then
        return reply(1, 0, 0)
    end
    if tpm_limit > 0 and (tpm_used > tpm_limit or amount > tpm_limit - tpm_used) then
        return reply(2, 0, 0)
    end
    rpm_used = rpm_used + reserve_rpm
    if tpm_limit > 0 then tpm_used = tpm_used + amount end
    redis.call('HSET', bucket_key, 'rpm', rpm_used, 'tpm', tpm_used)
    redis.call('HSET', marker_key, 'state', 'reserved', 'reserved_rpm', reserve_rpm,
        'reserved_tpm', amount, 'charged_tpm', amount)
    refresh_expiry()
    return reply(0, amount, amount)
end

if marker_fields == 0 then
    return reply(6, 0, 0)
end
local state = marker[1]
local reserved_rpm = tonumber(marker[2])
local reserved_tpm = tonumber(marker[3])
local charged_tpm = tonumber(marker[4])
if (reserved_rpm ~= 0 and reserved_rpm ~= 1)
    or not valid_nonnegative_integer(reserved_tpm) or not valid_nonnegative_integer(charged_tpm) then
    return redis.error_reply('corrupt credential reservation marker')
end

if operation == 2 then
    if state == 'reconciled' and charged_tpm == amount then
        refresh_expiry()
        return reply(3, charged_tpm, 0)
    end
    if state ~= 'reserved' then
        refresh_expiry()
        return reply(6, charged_tpm, 0)
    end
    local delta = 0
    local new_tpm = 0
    if amount >= reserved_tpm then
        delta = amount - reserved_tpm
        if delta > max_safe - tpm_used then
            return redis.error_reply('credential reservation reconciliation overflow')
        end
        new_tpm = tpm_used + delta
    else
        delta = amount - reserved_tpm
        local reduction = reserved_tpm - amount
        if reduction > tpm_used then
            return redis.error_reply('credential reservation reconciliation underflow')
        end
        new_tpm = tpm_used - reduction
    end
    tpm_used = new_tpm
    redis.call('HSET', bucket_key, 'tpm', tpm_used)
    redis.call('HSET', marker_key, 'state', 'reconciled', 'charged_tpm', amount)
    refresh_expiry()
    return reply(4, amount, delta)
end

if state == 'released' then
    refresh_expiry()
    return reply(3, 0, 0)
end
if state ~= 'reserved' then
    refresh_expiry()
    return reply(6, charged_tpm, 0)
end
if rpm_used < reserved_rpm or tpm_used < reserved_tpm then
    return redis.error_reply('credential reservation release underflow')
end
rpm_used = rpm_used - reserved_rpm
tpm_used = tpm_used - reserved_tpm
redis.call('HSET', bucket_key, 'rpm', rpm_used, 'tpm', tpm_used)
redis.call('HSET', marker_key, 'state', 'released', 'charged_tpm', 0)
refresh_expiry()
return reply(5, 0, -reserved_tpm)
"#;

/// Sliding Window Counter — single HASH key implementation.
///
/// Uses weighted average of current + previous fixed window to approximate
/// a true sliding window. Prevents the boundary burst problem of fixed windows.
///
/// HASH fields: `cur` (current window count), `prev` (previous window count),
/// `cur_ts` (current window start timestamp in ms).
///
/// Window rotation happens atomically within the script — no external timer needed.
///
/// KEYS[1] = sliding window hash key
/// ARGV[1] = rate limit
/// ARGV[2] = window size in milliseconds
/// ARGV[3] = current timestamp in milliseconds
/// ARGV[4] = cost (usually 1)
///
/// Returns: {allowed(0/1), current_count, remaining, retry_ms}
pub const SLIDING_WINDOW_SCRIPT: &str = r#"
local key = KEYS[1]
local rate = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local cost = tonumber(ARGV[4])

local window_start = now - (now % window)

local data = redis.call('HMGET', key, 'cur', 'prev', 'cur_ts')
local cur = tonumber(data[1]) or 0
local prev = tonumber(data[2]) or 0
local cur_ts = tonumber(data[3]) or 0

if cur_ts < window_start then
    if cur_ts >= window_start - window then
        prev = cur
    else
        prev = 0
    end
    cur = 0
    cur_ts = window_start
end

local elapsed = now - window_start
local weight = (window - elapsed) / window
local estimated = prev * weight + cur

if estimated + cost > rate then
    local ttl_ms = window - elapsed
    return {0, math.ceil(estimated), 0, ttl_ms}
end

cur = cur + cost
redis.call('HMSET', key, 'cur', cur, 'prev', prev, 'cur_ts', cur_ts)
redis.call('PEXPIRE', key, window * 2)

estimated = prev * weight + cur
local remaining = math.max(0, rate - math.ceil(estimated))
return {1, math.ceil(estimated), remaining, 0}
"#;

/// Fixed Window Counter — simple STRING key with INCRBY.
///
/// Resets at each window boundary. Simple but has the boundary burst problem
/// where 2x rate can pass at the boundary between two adjacent windows.
///
/// KEYS[1] = window key (includes window_id in the key name)
/// ARGV[1] = rate limit
/// ARGV[2] = window size in milliseconds
/// ARGV[3] = cost
///
/// Returns: {allowed(0/1), current_count, remaining, retry_ms}
pub const FIXED_WINDOW_SCRIPT: &str = r#"
local key = KEYS[1]
local rate = tonumber(ARGV[1])
local window_ms = tonumber(ARGV[2])
local cost = tonumber(ARGV[3])

local current = tonumber(redis.call('GET', key)) or 0

if current + cost > rate then
    local ttl = redis.call('PTTL', key)
    if ttl < 0 then ttl = window_ms end
    return {0, current, 0, ttl}
end

local new_count = redis.call('INCRBY', key, cost)
if new_count == cost then
    redis.call('PEXPIRE', key, window_ms)
end

return {1, new_count, math.max(0, rate - new_count), 0}
"#;

/// Token Bucket — HASH key with `tokens`, `ts`, and `refill_remainder` fields.
///
/// Allows controlled bursts up to bucket capacity (= rate).
/// Tokens are refilled based on elapsed time since last check.
///
/// KEYS[1] = bucket hash key
/// ARGV[1] = max_tokens (= rate = bucket capacity)
/// ARGV[2] = refill_rate (= rate, tokens replenished per interval)
/// ARGV[3] = interval_ms
/// ARGV[4] = now_ms
/// ARGV[5] = cost
///
/// Returns: {allowed(0/1), current_tokens, remaining, retry_ms}
pub const TOKEN_BUCKET_SCRIPT: &str = r#"
local key = KEYS[1]
local max_tokens = tonumber(ARGV[1])
local refill_rate = tonumber(ARGV[2])
local interval_ms = tonumber(ARGV[3])
local now = tonumber(ARGV[4])
local cost = tonumber(ARGV[5])

local data = redis.call('HMGET', key, 'tokens', 'ts', 'refill_remainder')
local tokens = tonumber(data[1])
local last_ts = tonumber(data[2])
local refill_remainder = tonumber(data[3]) or 0

if tokens == nil then
    tokens = max_tokens
    last_ts = now
    refill_remainder = 0
end

if now >= last_ts then
    local elapsed = now - last_ts
    if elapsed >= interval_ms then
        tokens = max_tokens
        refill_remainder = 0
        last_ts = now
    else
        local refill_numerator = elapsed * refill_rate + refill_remainder
        local refill = math.floor(refill_numerator / interval_ms)
        refill_remainder = refill_numerator - refill * interval_ms
        tokens = tokens + refill
        last_ts = now
        if tokens >= max_tokens then
            tokens = max_tokens
            refill_remainder = 0
        end
    end
end

if tokens >= cost then
    tokens = tokens - cost
    redis.call('HMSET', key, 'tokens', tokens, 'ts', last_ts, 'refill_remainder', refill_remainder)
    redis.call('PEXPIRE', key, interval_ms * 2)
    return {1, tokens, tokens, 0}
else
    local needed = cost - tokens
    local required_numerator = needed * interval_ms - refill_remainder
    local retry_ms = math.floor((required_numerator - 1) / refill_rate) + 1
    redis.call('HMSET', key, 'tokens', tokens, 'ts', last_ts, 'refill_remainder', refill_remainder)
    redis.call('PEXPIRE', key, interval_ms * 2)
    return {0, tokens, 0, retry_ms}
end
"#;

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/common/redis_rate_limiter/scripts_test.rs"]
mod tests;
