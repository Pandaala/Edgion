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
//! - **DNS failure keeps the last good answer** (Envoy behavior, bounded as in
//!   RFC 8767 serve-stale): a transient refresh failure keeps serving the stale
//!   answer and slides its stale window to `now + stale_ttl`, so an answer that
//!   is still requested survives a resolver outage, while an unused one still
//!   expires. The answer is never served more than `max_stale` past its
//!   freshness: a deleted record (which the System resolver reports as a
//!   transient error) stops being dialed within the record TTL plus `max_stale`.
//!   Refreshes back off exponentially (`negative_ttl` doubling up to
//!   `max_negative_ttl`). An authoritative empty answer (`NoAddresses`) drops it.
//! - **Cold miss**: same-key callers serialize on a per-key flight mutex and
//!   re-check the cache after acquiring it, so one resolver call (successful or
//!   not) serves them all. A cancelled leader just releases the mutex; the next
//!   waiter takes over. An answer whose refresh is still running after its stale
//!   window is served to cold callers instead of starting a second resolution.
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
/// Default ceiling on how long past its freshness a last good answer is served
/// while the resolver keeps failing (the RFC 8767 "maximum stale timer",
/// shortened from its resolver-oriented 1-3 days because a gateway keeps
/// dialing the answer: a deleted or reassigned destination must stop receiving
/// traffic within minutes).
const MAX_STALE_ANSWER_AGE: Duration = Duration::from_secs(300);

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
    /// Ceiling on serving an answer past its freshness while refreshes fail.
    max_stale: Duration,
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
            max_stale: MAX_STALE_ANSWER_AGE,
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

    /// LRU retention of an answer: until its stale limit plus one more stale
    /// window. An answer stays servable to cold lookups while its refresh runs,
    /// up to the stale limit, and a refresh may outlive the 30 s stale window
    /// (HTTPDNS timeouts plus fallback), so retention must cover the limit.
    /// Servability is decided by the entry's own deadlines, never by retention.
    fn answer_retention(&self, stale_limit: Instant, now: Instant) -> Duration {
        stale_limit
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
/// while its resolver keeps failing, up to its stale limit). `in_flight_keys`
/// lists at most a bounded sample of live flights as `policy|name`.
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
    /// Hard end of serving this answer: `fresh_until + max_stale`. Failed
    /// refreshes slide `stale_until` but never past this point.
    stale_limit: Instant,
    refresh_blocked_until: Option<Instant>,
    /// Consecutive failed refreshes since this answer was resolved.
    failures: u32,
    refreshing: AtomicBool,
}

impl AnswerEntry {
    /// Servable to a cold lookup: inside its stale window, or past it while
    /// its own claimed refresh is still running (bounded by the stale limit).
    fn is_servable(&self, now: Instant) -> bool {
        now < self.stale_until || (now < self.stale_limit && self.refreshing.load(Ordering::Acquire))
    }
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

/// A stale entry's refresh claim. A completed refresh always replaces the
/// entry, so its claim stays taken and a reader still holding the retired entry
/// cannot start a second refresh. A refresh cancelled by its caller's deadline
/// (common when the resolver hangs longer than an outbound connect timeout)
/// counts as a failed refresh: the last good answer is kept, its window slides
/// and the next refresh backs off, so the last-good guarantee does not depend
/// on callers outliving the resolver.
struct RefreshClaim<'a, 'q> {
    cache: &'a DnsAnswerCache,
    query: &'a CacheQuery<'q>,
    entry: &'a Arc<Entry>,
    policy: &'a DnsCachePolicy,
    armed: bool,
}

impl RefreshClaim<'_, '_> {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for RefreshClaim<'_, '_> {
    fn drop(&mut self) {
        if self.armed {
            self.cache.abandon_refresh(self.query, self.entry, self.policy);
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
                    // Read before claiming so readers of an entry under refresh
                    // do not each write the shared flag.
                    if blocked
                        || answer.refreshing.load(Ordering::Relaxed)
                        || answer.refreshing.swap(true, Ordering::AcqRel)
                    {
                        return Ok(answer.addrs.to_vec());
                    }
                    let claim = RefreshClaim {
                        cache: self,
                        query: &query,
                        entry,
                        policy,
                        armed: true,
                    };
                    let result = resolver(spec, name, query_policy).await;
                    claim.disarm();
                    return self.complete(&query, Some(entry), true, result, policy);
                }
                // Past its stale window with a refresh still running: serve it
                // without touching the flight registry (see the cold re-check).
                Entry::Answer(answer) if now < answer.stale_limit && answer.refreshing.load(Ordering::Acquire) => {
                    return Ok(answer.addrs.to_vec())
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
                // Inside the stale window, or with a refresh still in progress
                // (completed refreshes replace the entry, cancelled ones store
                // a replacement or clear the flag): the claimed refresh keeps
                // the answer on failure, so serve it rather than racing it.
                Entry::Answer(answer) if answer.is_servable(now) => return Ok(answer.addrs.to_vec()),
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
                    let stale_limit = fresh_until.checked_add(policy.max_stale).unwrap_or(fresh_until);
                    let stale_until = fresh_until
                        .checked_add(policy.stale_ttl)
                        .unwrap_or(fresh_until)
                        .min(stale_limit);
                    self.store(
                        query,
                        Entry::Answer(AnswerEntry {
                            addrs: Arc::from(answer.addrs.as_slice()),
                            fresh_until,
                            stale_until,
                            stale_limit,
                            refresh_blocked_until: None,
                            failures: 0,
                            refreshing: AtomicBool::new(false),
                        }),
                        policy.answer_retention(stale_limit, now),
                    );
                }
                return Ok(answer.addrs);
            }
            Err(error) => error,
        };
        let mut failures = 1;
        // An entry evicted while its refresh ran is still the last good answer
        // this lookup claimed: treat it as current so the failure keeps it.
        let current = self
            .entries
            .get(query)
            .or_else(|| previous.filter(|_| previous_servable).cloned());
        if let Some(current) = current {
            match current.as_ref() {
                Entry::Answer(answer) => {
                    let same = previous.is_some_and(|previous| Arc::ptr_eq(&current, previous));
                    if !same && answer.is_servable(now) {
                        // A newer answer was written while this lookup ran and
                        // is still servable (inside its stale window, or under
                        // its own running refresh): it wins over this failure.
                        return Ok(answer.addrs.to_vec());
                    }
                    if same
                        && (now < answer.stale_until || previous_servable)
                        && error != DnsError::NoAddresses
                        && now < answer.stale_limit
                    {
                        // Transient failure: keep the last good answer. An
                        // authoritative empty answer, or an answer past its
                        // stale limit, falls through and is replaced.
                        return Ok(self.keep_after_failure(query, answer, now, policy));
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

    /// Keep `answer` after a failed or abandoned refresh: slide its stale
    /// window (up to its stale limit) while it is requested, count the
    /// failure and back off the next refresh. Returns the served addresses.
    fn keep_after_failure(
        &self,
        query: &CacheQuery<'_>,
        answer: &AnswerEntry,
        now: Instant,
        policy: &DnsCachePolicy,
    ) -> Vec<IpAddr> {
        let failures = answer.failures.saturating_add(1);
        let stale_until = now
            .checked_add(policy.stale_ttl)
            .map_or(answer.stale_until, |slid| slid.max(answer.stale_until))
            .min(answer.stale_limit);
        self.store(
            query,
            Entry::Answer(AnswerEntry {
                addrs: answer.addrs.clone(),
                fresh_until: answer.fresh_until,
                stale_until,
                stale_limit: answer.stale_limit,
                refresh_blocked_until: now.checked_add(policy.backoff(failures)),
                failures,
                refreshing: AtomicBool::new(false),
            }),
            policy.answer_retention(answer.stale_limit, now),
        );
        answer.addrs.to_vec()
    }

    /// A claimed refresh was cancelled before its resolver returned. Treat it
    /// as a transient failure of the claimed answer when that answer is still
    /// current (or was evicted meanwhile) and within its stale limit; otherwise
    /// only release the claim. Never writes a negative entry: no resolver
    /// outcome was observed.
    fn abandon_refresh(&self, query: &CacheQuery<'_>, claimed: &Arc<Entry>, policy: &DnsCachePolicy) {
        let Entry::Answer(answer) = claimed.as_ref() else {
            return;
        };
        let now = self.now();
        let current = self.entries.get(query);
        let still_claimed = current.as_ref().is_none_or(|current| Arc::ptr_eq(current, claimed));
        if still_claimed && now < answer.stale_limit {
            self.keep_after_failure(query, answer, now, policy);
        } else {
            answer.refreshing.store(false, Ordering::Release);
        }
    }

    fn store(&self, query: &CacheQuery<'_>, entry: Entry, retain_for: Duration) {
        if retain_for.is_zero() {
            return;
        }
        self.entries.insert(query.to_key(), Arc::new(entry), Some(retain_for));
    }

    /// Counts only, for an instance whose names come from requests: listing
    /// them on an admin endpoint would echo client-supplied data.
    pub fn stats_counts_only(&self) -> DnsAnswerCacheStats {
        DnsAnswerCacheStats {
            entries: self.entries.len(),
            in_flight: self.flights.len(),
            in_flight_keys: Vec::new(),
        }
    }

    pub fn stats(&self) -> DnsAnswerCacheStats {
        DnsAnswerCacheStats {
            entries: self.entries.len(),
            in_flight: self.flights.len(),
            // List `policy|name` only: the resolver fingerprint can carry
            // provider URL configuration and adds nothing to a residue check.
            in_flight_keys: self
                .flights
                .keys(MAX_LISTED_FLIGHT_KEYS)
                .into_iter()
                .map(|key| match key.match_indices('|').nth(1) {
                    Some((end, _)) => key[..end].to_string(),
                    None => key,
                })
                .collect(),
        }
    }
}

/// Process-wide cache shared by every `EgressResolver` built without an explicit
/// cache instance (operator-configured egress destinations).
pub fn egress_answer_cache() -> &'static DnsAnswerCache {
    static CACHE: LazyLock<DnsAnswerCache> =
        LazyLock::new(|| DnsAnswerCache::new(EGRESS_CACHE_CAPACITY, EGRESS_CACHE_SHARDS));
    &CACHE
}

#[cfg(test)]
#[path = "../../tests/unit/dns/cache_test.rs"]
mod tests;
