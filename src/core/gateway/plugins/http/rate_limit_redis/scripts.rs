//! Redis Lua scripts for atomic rate limiting.
//!
//! All rate limit checks are executed atomically via Redis Lua scripts (EVAL/EVALSHA).
//! Each script operates on a single KEYS[1] for Redis Cluster compatibility (no CROSSSLOT).
//!
//! ## Return Value Convention
//! All scripts return: `{allowed(0/1), current_count, remaining, retry_ms}`
//!
//! ## EVALSHA Optimization
//! At runtime, scripts are loaded via SCRIPT LOAD and invoked via EVALSHA.
//! On NOSCRIPT (Redis restart / SCRIPT FLUSH), automatic fallback to EVAL.

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

/// Token Bucket — HASH key with `tokens` and `ts` fields.
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

local data = redis.call('HMGET', key, 'tokens', 'ts')
local tokens = tonumber(data[1])
local last_ts = tonumber(data[2])

if tokens == nil then
    tokens = max_tokens
    last_ts = now
end

local elapsed = math.max(0, now - last_ts)
local refill = math.floor(elapsed * refill_rate / interval_ms)
tokens = math.min(max_tokens, tokens + refill)
if refill > 0 then last_ts = now end

if tokens >= cost then
    tokens = tokens - cost
    redis.call('HMSET', key, 'tokens', tokens, 'ts', last_ts)
    redis.call('PEXPIRE', key, interval_ms * 2)
    return {1, tokens, tokens, 0}
else
    local needed = cost - tokens
    local retry_ms = math.ceil(needed * interval_ms / refill_rate)
    redis.call('HMSET', key, 'ts', last_ts)
    redis.call('PEXPIRE', key, interval_ms * 2)
    return {0, tokens, 0, retry_ms}
end
"#;
