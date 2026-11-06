//! Shared, bounded, TTL-honoring DNS answer cache.
//!
//! One cache instance fronts [`resolve_answer`] for a class of consumers. It
//! stores raw resolver answers (consumers still apply their own egress
//! classification on every use) keyed by the complete answer identity:
//! resolver fingerprint, queried name and query policy.
//!
//! Lifecycle of one key:
//!
//! - **Fresh** for the record TTL reported by the resolver, clamped to
//!   `[MIN_ANSWER_TTL, max_ttl]`, or for the consumer's fallback TTL (capped at
//!   `max_ttl`) when the resolver reports none (System resolver, TTL-less
//!   HTTPDNS). The 5 s floor matches Envoy's `dns_min_refresh_rate`.
//! - **Stale** for `stale_ttl` afterwards. Exactly one request (the first to
//!   claim the entry's refresh flag) refreshes inline; every other request is
//!   served the stale answer without waiting.
//! - **DNS failure keeps the last good answer** (Envoy behavior): a transient
//!   refresh failure keeps serving the stale answer and slides its stale window
//!   to `now + stale_ttl`, so an answer that is still requested survives a
//!   resolver outage of any length, while an unused one still expires. Refreshes
//!   back off exponentially (`negative_ttl` doubling up to `max_negative_ttl`).
//!   An authoritative empty answer (`NoAddresses`) drops it.
//! - **Cold miss**: same-key callers serialize on a per-key flight mutex and
//!   re-check the cache after acquiring it, so one resolver call serves them all.
//!   A cancelled leader just releases the mutex; the next waiter takes over.
//! - **Negative**: a failure with no servable stale answer is cached for the
//!   current back-off (`negative_ttl` doubling per consecutive failure, up to
//!   `max_negative_ttl`).
//!
//! The flight mutex is held across the DNS round trip. It guards no data and only
//! serializes same-key cold misses (the same shape as the OIDC JWKS provider);
//! see `skills/04-review/concurrency/intentional-design/single-flight-mutex-across-io.md`
//! for the recorded exemption from design rule V-1.

use std::borrow::{Borrow, Cow};
use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use edgion_algo::cache::ShardedLruCache;
use serde::Serialize;

use super::{resolve_answer, strip_brackets, DnsAnswer, DnsError, DnsQueryPolicy, ResolverSpec};
use crate::singleflight::{FlightRegistry, MAX_FLIGHT_REGISTERED_KEYS};

/// Floor applied to a reported record TTL so TTL-0 or very short records cannot
/// turn the cache into a resolve-per-request loop (Envoy `dns_min_refresh_rate`).
const MIN_ANSWER_TTL: Duration = Duration::from_secs(5);
/// Default ceiling on a reported record TTL.
const MAX_ANSWER_TTL: Duration = Duration::from_secs(300);
/// Default window during which an expired answer is still served.
const STALE_ANSWER_WINDOW: Duration = Duration::from_secs(30);
/// Default first back-off after a failure (cached failure or refresh block).
const NEGATIVE_ANSWER_TTL: Duration = Duration::from_secs(1);
/// Default ceiling of the exponential failure back-off (10x the base, as
/// Envoy's `dns_failure_refresh_rate` default).
const MAX_NEGATIVE_ANSWER_TTL: Duration = Duration::from_secs(10);

const EGRESS_CACHE_CAPACITY: usize = 4096;
const EGRESS_CACHE_SHARDS: usize = 16;
/// Longest name that is cached (a 253-byte DNS name plus a trailing dot).
/// Longer names cannot resolve and are passed through uncached so request- or
/// upstream-influenced junk cannot pin large keys in the cache.
const MAX_CACHED_NAME_LEN: usize = 254;
/// Upper bound on flight keys listed by [`DnsAnswerCache::stats`].
const MAX_LISTED_FLIGHT_KEYS: usize = 64;

/// Per-consumer freshness policy. Deadlines are stamped when an answer is
/// written, from the writer's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DnsCachePolicy {
    /// Freshness of an answer without a reported TTL. Zero disables caching.
    fallback_ttl: Duration,
    max_ttl: Duration,
    stale_ttl: Duration,
    /// First failure back-off; doubles per consecutive failure.
    negative_ttl: Duration,
    /// Ceiling of the failure back-off.
    max_negative_ttl: Duration,
}

impl DnsCachePolicy {
    /// Policy with the default bounds and the given fallback TTL.
    pub const fn new(fallback_ttl: Duration) -> Self {
        Self {
            fallback_ttl,
            max_ttl: MAX_ANSWER_TTL,
            stale_ttl: STALE_ANSWER_WINDOW,
            negative_ttl: NEGATIVE_ANSWER_TTL,
            max_negative_ttl: MAX_NEGATIVE_ANSWER_TTL,
        }
    }

    /// Back-off after `failures` consecutive failures (`failures >= 1`).
    fn backoff(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.negative_ttl
            .saturating_mul(1 << doublings)
            .min(self.max_negative_ttl.max(self.negative_ttl))
    }

    /// Freshness of an answer whose resolver reports no record TTL.
    pub fn fallback_ttl(&self) -> Duration {
        self.fallback_ttl
    }

    /// A zero fallback TTL disables the cache for this consumer entirely.
    fn is_disabled(&self) -> bool {
        self.fallback_ttl.is_zero()
    }

    /// LRU retention of an answer: its stale window plus one more window, so a
    /// refresh that is still running when the window ends finds the entry.
    /// Servability is decided by the entry's own deadlines, never by retention.
    fn answer_retention(&self, stale_until: Instant, now: Instant) -> Duration {
        stale_until
            .saturating_duration_since(now)
            .saturating_add(self.stale_ttl)
    }

    /// Freshness of one answer. Every answer, with or without a reported TTL,
    /// is capped at `max_ttl`.
    fn fresh_for(&self, ttl: Option<Duration>) -> Duration {
        match ttl {
            Some(ttl) => ttl.max(MIN_ANSWER_TTL).min(self.max_ttl),
            None => self.fallback_ttl.min(self.max_ttl),
        }
    }
}

/// Store-stats projection. `in_flight` must return to zero once requests
/// settle; `entries` is bounded by capacity (a still-requested answer can stay
/// while its resolver keeps failing). `in_flight_keys` lists at most a bounded
/// sample of live flight keys.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DnsAnswerCacheStats {
    pub entries: usize,
    pub in_flight: usize,
    pub in_flight_keys: Vec<String>,
}

/// Shared fieldwise identity for owned entries and allocation-free lookups.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheQuery<'a> {
    fingerprint: Cow<'a, str>,
    name: Cow<'a, str>,
    policy: DnsQueryPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey(CacheQuery<'static>);

impl<'a> Borrow<CacheQuery<'a>> for CacheKey {
    fn borrow(&self) -> &CacheQuery<'a> {
        &self.0
    }
}

impl CacheQuery<'_> {
    fn to_key(&self) -> CacheKey {
        CacheKey(CacheQuery {
            fingerprint: Cow::Owned(self.fingerprint.to_string()),
            name: Cow::Owned(self.name.to_string()),
            policy: self.policy,
        })
    }

    fn flight_key(&self) -> String {
        format!("{:?}|{}|{}", self.policy, self.name, self.fingerprint)
    }
}

#[derive(Debug)]
struct AnswerEntry {
    addrs: Arc<[IpAddr]>,
    fresh_until: Instant,
    stale_until: Instant,
    refresh_blocked_until: Option<Instant>,
    /// Consecutive failed refreshes since this answer was resolved.
    failures: u32,
    refreshing: AtomicBool,
}

#[derive(Debug)]
enum Entry {
    Answer(AnswerEntry),
    Negative {
        error: DnsError,
        until: Instant,
        /// Consecutive failures that produced this entry.
        failures: u32,
    },
}

/// Releases a stale entry's refresh claim if the refresh is cancelled. A
/// completed refresh always replaces the entry, so its claim stays taken and a
/// reader still holding the retired entry cannot start a second refresh.
struct RefreshClaim<'a> {
    flag: &'a AtomicBool,
    armed: bool,
}

impl<'a> RefreshClaim<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        Self { flag, armed: true }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for RefreshClaim<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::Release);
        }
    }
}

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Bounded DNS answer cache. See the module documentation for the lifecycle.
pub struct DnsAnswerCache {
    entries: ShardedLruCache<CacheKey, Arc<Entry>>,
    flights: FlightRegistry,
    clock: Clock,
}

impl DnsAnswerCache {
    /// Build a cache bounded to `capacity` entries across `shards` shards
    /// (`ShardedLruCache::new` requires `shards <= capacity`).
    pub fn new(capacity: usize, shards: usize) -> Self {
        Self::with_clock(capacity, shards, Arc::new(Instant::now))
    }

    /// Build a cache that reads time from `clock` (deterministic time source).
    fn with_clock(capacity: usize, shards: usize, clock: Clock) -> Self {
        Self {
            entries: ShardedLruCache::new(capacity, shards),
            flights: FlightRegistry::new(MAX_FLIGHT_REGISTERED_KEYS)
                .expect("the registry ceiling is a valid registry limit"),
            clock,
        }
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    /// Resolve through the canonical resolver with caching.
    pub async fn lookup(
        &self,
        spec: &ResolverSpec,
        name: &str,
        query_policy: DnsQueryPolicy,
        policy: &DnsCachePolicy,
    ) -> Result<Vec<IpAddr>, DnsError> {
        self.lookup_with(spec, name, query_policy, policy, resolve_answer).await
    }

    /// Resolve through `resolver` with caching. IP literals and a disabled
    /// policy call `resolver` directly and store nothing.
    pub async fn lookup_with<'a, R, Fut>(
        &self,
        spec: &'a ResolverSpec,
        name: &'a str,
        query_policy: DnsQueryPolicy,
        policy: &DnsCachePolicy,
        resolver: R,
    ) -> Result<Vec<IpAddr>, DnsError>
    where
        R: FnOnce(&'a ResolverSpec, &'a str, DnsQueryPolicy) -> Fut,
        Fut: Future<Output = Result<DnsAnswer, DnsError>>,
    {
        if policy.is_disabled() || name.len() > MAX_CACHED_NAME_LEN || strip_brackets(name).parse::<IpAddr>().is_ok() {
            return resolver(spec, name, query_policy).await.map(|answer| answer.addrs);
        }
        let fingerprint = spec.fingerprint();
        let query = CacheQuery {
            fingerprint: Cow::Borrowed(fingerprint.as_ref()),
            name: Cow::Borrowed(name),
            policy: query_policy,
        };

        let now = self.now();
        let previous = self.entries.get(&query);
        if let Some(entry) = previous.as_ref() {
            match entry.as_ref() {
                Entry::Answer(answer) if now < answer.fresh_until => return Ok(answer.addrs.to_vec()),
                Entry::Answer(answer) if now < answer.stale_until => {
                    let blocked = answer.refresh_blocked_until.is_some_and(|until| now < until);
                    if blocked || answer.refreshing.swap(true, Ordering::AcqRel) {
                        return Ok(answer.addrs.to_vec());
                    }
                    let claim = RefreshClaim::new(&answer.refreshing);
                    let result = resolver(spec, name, query_policy).await;
                    claim.disarm();
                    return self.complete(&query, Some(entry), true, result, policy);
                }
                Entry::Negative { error, until, .. } if now < *until => return Err(error.clone()),
                _ => {}
            }
        }

        let flight = self.flights.get_or_create(&query.flight_key());
        let _turn = flight.lock().lock().await;
        let now = self.now();
        let previous = self.entries.get(&query);
        if let Some(entry) = previous.as_ref() {
            match entry.as_ref() {
                Entry::Answer(answer) if now < answer.stale_until => return Ok(answer.addrs.to_vec()),
                Entry::Negative { error, until, .. } if now < *until => return Err(error.clone()),
                _ => {}
            }
        }
        let result = resolver(spec, name, query_policy).await;
        self.complete(&query, previous.as_ref(), false, result, policy)
    }

    /// Apply the write rule for one resolver outcome and return what the
    /// caller is served. `previous` is the entry the lookup started from;
    /// `previous_servable` says it was a servable answer when the refresh was
    /// claimed, so a refresh that outlives the stale window still keeps it.
    fn complete(
        &self,
        query: &CacheQuery<'_>,
        previous: Option<&Arc<Entry>>,
        previous_servable: bool,
        result: Result<DnsAnswer, DnsError>,
        policy: &DnsCachePolicy,
    ) -> Result<Vec<IpAddr>, DnsError> {
        let now = self.now();
        let error = match result {
            Ok(answer) => {
                if let Some(fresh_until) = now.checked_add(policy.fresh_for(answer.ttl)) {
                    let stale_until = fresh_until.checked_add(policy.stale_ttl).unwrap_or(fresh_until);
                    self.store(
                        query,
                        Entry::Answer(AnswerEntry {
                            addrs: Arc::from(answer.addrs.as_slice()),
                            fresh_until,
                            stale_until,
                            refresh_blocked_until: None,
                            failures: 0,
                            refreshing: AtomicBool::new(false),
                        }),
                        policy.answer_retention(stale_until, now),
                    );
                }
                return Ok(answer.addrs);
            }
            Err(error) => error,
        };
        let mut failures = 1;
        if let Some(current) = self.entries.get(query) {
            match current.as_ref() {
                Entry::Answer(answer) => {
                    let same = previous.is_some_and(|previous| Arc::ptr_eq(&current, previous));
                    if now < answer.stale_until || (same && previous_servable) {
                        if !same {
                            // A newer answer was written while this lookup ran:
                            // it wins over this failure and is left untouched.
                            return Ok(answer.addrs.to_vec());
                        }
                        if error != DnsError::NoAddresses {
                            // Transient failure: keep the last good answer,
                            // slide its stale window while it is requested and
                            // back off refreshes. An authoritative empty answer
                            // falls through and replaces it.
                            let failures = answer.failures.saturating_add(1);
                            let stale_until = now
                                .checked_add(policy.stale_ttl)
                                .map_or(answer.stale_until, |slid| slid.max(answer.stale_until));
                            let addrs = answer.addrs.to_vec();
                            self.store(
                                query,
                                Entry::Answer(AnswerEntry {
                                    addrs: answer.addrs.clone(),
                                    fresh_until: answer.fresh_until,
                                    stale_until,
                                    refresh_blocked_until: now.checked_add(policy.backoff(failures)),
                                    failures,
                                    refreshing: AtomicBool::new(false),
                                }),
                                policy.answer_retention(stale_until, now),
                            );
                            return Ok(addrs);
                        }
                    }
                    failures = answer.failures.saturating_add(1);
                }
                Entry::Negative {
                    failures: previous_failures,
                    ..
                } => failures = previous_failures.saturating_add(1),
            }
        }
        let backoff = policy.backoff(failures);
        if let Some(until) = now.checked_add(backoff) {
            self.store(
                query,
                Entry::Negative {
                    error: error.clone(),
                    until,
                    failures,
                },
                // Retained past `until` so the next failure still sees the
                // consecutive-failure count and the back-off can grow.
                backoff.saturating_add(policy.max_negative_ttl),
            );
        }
        Err(error)
    }

    fn store(&self, query: &CacheQuery<'_>, entry: Entry, retain_for: Duration) {
        if retain_for.is_zero() {
            return;
        }
        self.entries.insert(query.to_key(), Arc::new(entry), Some(retain_for));
    }

    pub fn stats(&self) -> DnsAnswerCacheStats {
        DnsAnswerCacheStats {
            entries: self.entries.len(),
            in_flight: self.flights.len(),
            in_flight_keys: self.flights.keys(MAX_LISTED_FLIGHT_KEYS),
        }
    }
}

/// Process-wide cache shared by every `EgressResolver` in this process.
pub fn egress_answer_cache() -> &'static DnsAnswerCache {
    static CACHE: LazyLock<DnsAnswerCache> =
        LazyLock::new(|| DnsAnswerCache::new(EGRESS_CACHE_CAPACITY, EGRESS_CACHE_SHARDS));
    &CACHE
}

#[cfg(test)]
#[path = "../../tests/unit/dns/cache_test.rs"]
mod tests;
