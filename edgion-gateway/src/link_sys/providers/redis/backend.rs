//! Redis transport ownership. Business commands are never replayed after I/O failure.
use arc_swap::ArcSwapOption;
use futures::{stream, StreamExt};
use redis::{aio::MultiplexedConnection, Cmd, RedisResult, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

pub(super) fn unavailable() -> redis::RedisError {
    redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "Redis unavailable",
    ))
}
pub(super) fn timed_out() -> redis::RedisError {
    redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "Redis operation timed out",
    ))
}

pub(super) struct Member {
    pub connection: MultiplexedConnection,
    pub live: AtomicBool,
}

pub(super) struct MemberPool {
    pub members: Vec<Member>,
    next: AtomicUsize,
}

impl MemberPool {
    pub fn new(connections: Vec<MultiplexedConnection>) -> Self {
        Self {
            members: connections
                .into_iter()
                .map(|connection| Member {
                    connection,
                    live: AtomicBool::new(true),
                })
                .collect(),
            next: AtomicUsize::new(0),
        }
    }

    pub async fn exec(&self, cmd: &Cmd) -> RedisResult<Value> {
        let len = self.members.len();
        if len == 0 {
            return Err(unavailable());
        }
        let start = self.next.fetch_add(1, Ordering::Relaxed) % len;
        let member = (0..len)
            .map(|offset| &self.members[(start + offset) % len])
            .find(|member| member.live.load(Ordering::Relaxed))
            .ok_or_else(unavailable)?;
        let mut connection = member.connection.clone();
        let result = cmd.query_async(&mut connection).await;
        if result.as_ref().is_err_and(|e: &redis::RedisError| e.is_io_error()) {
            member.live.store(false, Ordering::Relaxed);
        }
        result
    }

    /// Collect each result independently before the shared probe's outer timeout.
    pub async fn probe(&self, timeout: Duration) -> usize {
        let connections: Vec<_> = self.members.iter().map(|member| member.connection.clone()).collect();
        let mut probes = stream::iter(
            connections
                .into_iter()
                .enumerate()
                .map(|(index, mut connection)| async move {
                    let live = tokio::time::timeout(timeout, redis::cmd("PING").query_async::<String>(&mut connection))
                        .await
                        .is_ok_and(|result| result.is_ok());
                    (index, live)
                }),
        )
        .buffer_unordered(64);
        let mut live = 0;
        while let Some((index, ok)) = probes.next().await {
            self.members[index].live.store(ok, Ordering::Relaxed);
            live += usize::from(ok);
        }
        live
    }
}

use super::{resolver::RedisEgressResolver, transport::Transport};
use edgion_resources::resources::link_sys::redis::RedisTopologyMode;
use redis::cluster_routing::{RoutingInfo, SingleNodeRoutingInfo};

pub(super) enum Snapshot {
    Pool {
        pool: MemberPool,
        primary: (String, u16),
        learned: Vec<(String, u16)>,
    },
    Cluster {
        connection: redis::cluster_async::ClusterConnection,
        primaries: Vec<(String, u16)>,
    },
}

pub(super) struct SentinelVote {
    /// The answering Sentinel's own run ID (`SENTINEL MYID`). One Sentinel
    /// reached through several addresses (configured DNS name, learned IP)
    /// still reports one ID, so it casts one vote.
    pub sentinel_id: String,
    pub primary: (String, u16),
    pub learned: Vec<(String, u16)>,
}

pub(super) struct RedisBackend {
    pub transport: Transport,
    pub snapshot: ArcSwapOption<Snapshot>,
    pub retired: AtomicBool,
    pub needs_recovery: AtomicBool,
    pub wake: tokio::sync::Notify,
    pub coverage: parking_lot::Mutex<(usize, usize)>,
    cursor: AtomicUsize,
    /// Identifies the newest `establish` attempt. An older attempt cannot publish
    /// after a newer one has started, including after retirement.
    establish_epoch: AtomicUsize,
    publication: parking_lot::Mutex<()>,
}

impl RedisBackend {
    pub fn new(transport: Transport) -> Self {
        let configured = if matches!(transport.mode, RedisTopologyMode::Cluster) {
            0
        } else {
            transport.pool_size
        };
        Self {
            transport,
            snapshot: ArcSwapOption::empty(),
            retired: AtomicBool::new(false),
            needs_recovery: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
            coverage: parking_lot::Mutex::new((0, configured)),
            cursor: AtomicUsize::new(0),
            establish_epoch: AtomicUsize::new(0),
            publication: parking_lot::Mutex::new(()),
        }
    }

    pub fn retire(&self) {
        let _publication = self.publication.lock();
        self.retired.store(true, Ordering::Release);
    }
    pub async fn establish(&self, resolver: RedisEgressResolver) -> RedisResult<()> {
        let epoch = self.establish_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let snapshot = match self.transport.mode {
            RedisTopologyMode::Cluster => {
                let seeds = self.cluster_candidate_addresses();
                let mut connection = self.transport.cluster(&seeds, resolver.clone()).await?;
                let primaries = match primaries_from_connection(&mut connection, &seeds).await {
                    Ok(primaries) => primaries,
                    Err(_) => self.cluster_primaries(resolver.clone()).await?,
                };
                Snapshot::Cluster { connection, primaries }
            }
            _ => {
                let (primary, learned) = if matches!(self.transport.mode, RedisTopologyMode::Sentinel) {
                    self.discover(resolver.clone()).await?
                } else {
                    (
                        self.transport.endpoints.first().cloned().ok_or_else(unavailable)?,
                        Vec::new(),
                    )
                };
                let previous = self.snapshot.load_full();
                let mut connections = Vec::new();
                if let Some(previous) = &previous {
                    if let Snapshot::Pool {
                        pool,
                        primary: old_primary,
                        ..
                    } = &**previous
                    {
                        if old_primary == &primary {
                            connections.extend(
                                pool.members
                                    .iter()
                                    .filter(|member| member.live.load(Ordering::Acquire))
                                    .map(|member| member.connection.clone()),
                            );
                        }
                    }
                }
                let missing = self.transport.pool_size.saturating_sub(connections.len());
                let attempts = stream::iter((0..missing).map(|_| {
                    let resolver = resolver.clone();
                    let primary = &primary;
                    async move { self.transport.connect(&primary.0, primary.1, false, resolver).await }
                }))
                .buffer_unordered(64)
                .collect::<Vec<_>>()
                .await;
                let (dialed, dial_error) = split_dial_results(attempts);
                connections.extend(dialed);
                if connections.is_empty() {
                    return Err(dial_error.unwrap_or_else(unavailable));
                }
                Snapshot::Pool {
                    pool: MemberPool::new(connections),
                    primary,
                    learned,
                }
            }
        };
        let _publication = self.publication.lock();
        if self.retired.load(Ordering::Acquire) || self.establish_epoch.load(Ordering::Acquire) != epoch {
            return Err(unavailable());
        }
        if let Snapshot::Cluster { primaries, .. } = &snapshot {
            if primaries.is_empty() {
                return Err(unavailable());
            }
        }
        *self.coverage.lock() = match &snapshot {
            Snapshot::Pool { pool, .. } => (pool.members.len(), self.transport.pool_size),
            Snapshot::Cluster { primaries, .. } => cluster_published_coverage(primaries.len()),
        };
        let incomplete =
            matches!(&snapshot, Snapshot::Pool { pool, .. } if pool.members.len() < self.transport.pool_size);
        self.snapshot.store(Some(Arc::new(snapshot)));
        self.needs_recovery.store(incomplete, Ordering::Release);
        Ok(())
    }

    /// The published Sentinel primary, if any. Standalone and Cluster have none.
    pub fn published_primary(&self) -> Option<(String, u16)> {
        if !matches!(self.transport.mode, RedisTopologyMode::Sentinel) {
            return None;
        }
        match self.snapshot.load_full().as_deref() {
            Some(Snapshot::Pool { primary, .. }) => Some(primary.clone()),
            _ => None,
        }
    }

    /// The published Cluster primary set, if any. Other modes have none.
    pub fn published_cluster_primaries(&self) -> Option<Vec<(String, u16)>> {
        match self.snapshot.load_full().as_deref() {
            Some(Snapshot::Cluster { primaries, .. }) => Some(primaries.clone()),
            _ => None,
        }
    }

    fn cluster_candidate_addresses(&self) -> Vec<(String, u16)> {
        let known = self.snapshot.load_full().and_then(|snapshot| match &*snapshot {
            Snapshot::Cluster { primaries, .. } => Some(primaries.clone()),
            Snapshot::Pool { .. } => None,
        });
        cluster_discovery_candidates(&self.transport.endpoints, known.as_deref().unwrap_or(&[]))
    }

    async fn cluster_primaries(&self, resolver: RedisEgressResolver) -> RedisResult<Vec<(String, u16)>> {
        let candidates = self.cluster_candidate_addresses();
        if candidates.is_empty() {
            return Err(unavailable());
        }
        let mut attempts = stream::iter(candidates.into_iter().map(|(host, port)| {
            let resolver = resolver.clone();
            async move {
                let mut connection = self.transport.connect(&host, port, false, resolver).await?;
                let value = redis::cmd("CLUSTER").arg("SLOTS").query_async(&mut connection).await?;
                parse_primaries(value, &host)
            }
        }))
        .buffer_unordered(CLUSTER_DISCOVERY_CONCURRENCY);
        let mut last_error = unavailable();
        while let Some(result) = attempts.next().await {
            match result {
                Ok(primaries) => return Ok(primaries),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    async fn discover(&self, resolver: RedisEgressResolver) -> RedisResult<((String, u16), Vec<(String, u16)>)> {
        let master = self.transport.master.clone().ok_or_else(unavailable)?;
        let snapshot = self.snapshot.load_full();
        let learned = match snapshot.as_deref() {
            Some(Snapshot::Pool { learned, .. }) => learned.as_slice(),
            _ => &[],
        };
        let seeds = sentinel_discovery_candidates(&self.transport.endpoints, learned);
        let mut attempts = stream::iter(seeds.into_iter().map(|(host, port)| {
            let resolver = resolver.clone();
            let master = master.clone();
            async move { self.sentinel_vote(&host, port, &master, resolver).await }
        }))
        .buffer_unordered(CLUSTER_DISCOVERY_CONCURRENCY);
        let mut results = Vec::new();
        while let Some(result) = attempts.next().await {
            results.push(result);
        }
        let (votes, vote_error) = split_dial_results(results);
        if votes.is_empty() {
            return Err(vote_error.unwrap_or_else(unavailable));
        }
        tally_sentinel_votes(votes).ok_or_else(unavailable)
    }

    async fn sentinel_vote(
        &self,
        host: &str,
        port: u16,
        master: &str,
        resolver: RedisEgressResolver,
    ) -> RedisResult<SentinelVote> {
        let mut sentinel = self.transport.connect(host, port, true, resolver.clone()).await?;
        let sentinel_id: String = redis::cmd("SENTINEL").arg("MYID").query_async(&mut sentinel).await?;
        if sentinel_id.is_empty() {
            return Err(unavailable());
        }
        let address: Vec<String> = redis::cmd("SENTINEL")
            .arg("get-master-addr-by-name")
            .arg(master)
            .query_async(&mut sentinel)
            .await?;
        let primary = parse_address(&address)?;
        let mut connection = self.transport.connect(&primary.0, primary.1, false, resolver).await?;
        let role: Value = redis::cmd("ROLE").query_async(&mut connection).await?;
        if !is_master(&role) {
            return Err(unavailable());
        }
        let learned = match redis::cmd("SENTINEL")
            .arg("sentinels")
            .arg(master)
            .query_async(&mut sentinel)
            .await
        {
            Ok(peers) => learned_sentinels(peers),
            Err(_) => Vec::new(),
        };
        Ok(SentinelVote {
            sentinel_id,
            primary,
            learned,
        })
    }

    pub async fn exec(&self, cmd: &Cmd, deadline: Instant, resolver: RedisEgressResolver) -> RedisResult<Value> {
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let operation = async {
            if self.retired.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let snapshot = self.snapshot.load_full().ok_or_else(unavailable)?;
            let mut result = match &*snapshot {
                Snapshot::Pool { pool, .. } => pool.exec(cmd).await,
                Snapshot::Cluster { connection, .. } => {
                    normalize_command_value(connection.clone().route_command(cmd.clone(), command_route(cmd)?).await)
                }
            };
            if matches!(&*snapshot, Snapshot::Cluster { .. }) {
                for _ in 0..self.transport.max_redirects {
                    let Some((host, port)) = moved_target(&result)? else {
                        break;
                    };
                    let mut connection = self.transport.connect(&host, port, false, resolver.clone()).await?;
                    result = normalize_command_value(cmd.query_async(&mut connection).await);
                }
            }
            if let Err(error) = &result {
                let _publication = self.publication.lock();
                let snapshot_current = self
                    .snapshot
                    .load_full()
                    .is_some_and(|current| Arc::ptr_eq(&current, &snapshot));
                if command_failure_requests_recovery(snapshot_current, error) {
                    self.needs_recovery.store(true, Ordering::Release);
                    self.wake.notify_one();
                }
            }
            result
        };
        timeout_at(deadline, operation).await.map_err(|_| timed_out())?
    }

    pub async fn probe(&self, resolver: RedisEgressResolver) -> bool {
        self.probe_view(resolver).await.healthy
    }

    /// Probe the published generation. `observed` is set only when this call
    /// still owns that generation, so a caller cannot treat a newer pool's
    /// coverage as PINGs this call performed.
    pub async fn probe_view(&self, _resolver: RedisEgressResolver) -> ProbeView {
        let Some(snapshot) = self.snapshot.load_full() else {
            return ProbeView {
                healthy: false,
                observed: None,
            };
        };
        let budget = self.transport.command_timeout.min(Duration::from_millis(500));
        let mut topology_changed = false;
        let (live, configured) = match &*snapshot {
            Snapshot::Pool { pool, .. } => {
                let count = pool.probe(budget).await;
                if matches!(self.transport.mode, RedisTopologyMode::Sentinel) {
                    let role = match tokio::time::timeout(budget, pool.exec(&redis::cmd("ROLE"))).await {
                        Ok(result) => RoleReply::Ready(result),
                        Err(_) => RoleReply::TimedOut,
                    };
                    sentinel_probe_coverage(count, self.transport.pool_size, classify_role(role))
                } else {
                    (count, self.transport.pool_size)
                }
            }
            Snapshot::Cluster { connection, primaries } => {
                let mut connection = connection.clone();
                let seeds = self.cluster_candidate_addresses();
                let refreshed = tokio::time::timeout(budget, primaries_from_connection(&mut connection, &seeds)).await;
                let probe_primaries = if let Ok(Ok(current)) = refreshed {
                    topology_changed = &current != primaries;
                    current
                } else {
                    primaries.clone()
                };
                let len = probe_primaries.len();
                let start = self.cursor.fetch_add(64, Ordering::Relaxed) % len.max(1);
                let mut probes = stream::iter((0..len).map(|offset| {
                    let (host, port) = probe_primaries[(start + offset) % len].clone();
                    let mut connection = connection.clone();
                    async move {
                        cluster_ping_succeeded(
                            tokio::time::timeout(
                                budget,
                                connection.route_command(
                                    redis::cmd("PING"),
                                    RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress { host, port }),
                                ),
                            )
                            .await,
                        )
                    }
                }))
                .buffer_unordered(64);
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut live = 0;
                while let Ok(Some(ok)) = timeout_at(deadline, probes.next()).await {
                    live += usize::from(ok);
                }
                (live, len)
            }
        };
        let _publication = self.publication.lock();
        let retired = self.retired.load(Ordering::Acquire);
        let snapshot_current = self
            .snapshot
            .load_full()
            .is_some_and(|current| Arc::ptr_eq(&current, &snapshot));
        let published_live = self.coverage.lock().0;
        let record = record_probe(
            retired,
            snapshot_current,
            published_live,
            (live, configured),
            topology_changed,
            probe_recovery_for(matches!(&*snapshot, Snapshot::Cluster { .. })),
        );
        if record.schedule_recovery {
            self.needs_recovery.store(true, Ordering::Release);
            self.wake.notify_one();
        }
        if let Some(coverage) = record.coverage {
            *self.coverage.lock() = coverage;
        }
        ProbeView {
            healthy: record.healthy,
            observed: record.coverage,
        }
    }
}

pub(super) struct ProbeView {
    pub healthy: bool,
    pub observed: Option<(usize, usize)>,
}

/// A Cluster generation is usable as soon as its connection and slot map exist.
/// One live connection keeps replacement on the handover path until this
/// snapshot's own probe replaces the count. An empty map stays unusable.
pub(super) fn cluster_published_coverage(primary_count: usize) -> (usize, usize) {
    (usize::from(primary_count > 0), primary_count)
}

/// Apply one probe only when it still observes the published snapshot.
/// A replaced snapshot must not rewrite coverage or schedule another recovery.
pub(super) struct ProbeRecord {
    pub healthy: bool,
    pub coverage: Option<(usize, usize)>,
    pub schedule_recovery: bool,
}

pub(super) enum ProbeRecovery {
    /// Standalone and Sentinel: any missing member schedules another establish.
    PartialMembers,
    /// Cluster: rebuild only when the slot map changes or no probed primary answers.
    UnreachableOrTopology,
}

pub(super) fn probe_recovery_for(cluster: bool) -> ProbeRecovery {
    if cluster {
        ProbeRecovery::UnreachableOrTopology
    } else {
        ProbeRecovery::PartialMembers
    }
}

pub(super) fn record_probe(
    retired: bool,
    snapshot_current: bool,
    published_live: usize,
    observed: (usize, usize),
    topology_changed: bool,
    recovery: ProbeRecovery,
) -> ProbeRecord {
    if retired || !snapshot_current {
        return ProbeRecord {
            healthy: !retired && published_live > 0,
            coverage: None,
            schedule_recovery: false,
        };
    }
    let (live, configured) = observed;
    let schedule_recovery = match recovery {
        ProbeRecovery::PartialMembers => topology_changed || live < configured,
        ProbeRecovery::UnreachableOrTopology => topology_changed || (configured > 0 && live == 0),
    };
    ProbeRecord {
        healthy: live > 0,
        coverage: Some(observed),
        schedule_recovery,
    }
}

/// Known primaries are tried before configured seeds, and only the first 16
/// unique known primaries are kept. Configured seeds are always retained, so a
/// removed seed cannot block refresh and a short configured list cannot be truncated.
pub(super) const MAX_KNOWN_CLUSTER_PRIMARIES: usize = 16;
const CLUSTER_DISCOVERY_CONCURRENCY: usize = 8;

pub(super) fn cluster_discovery_candidates(
    configured: &[(String, u16)],
    known_primaries: &[(String, u16)],
) -> Vec<(String, u16)> {
    let mut candidates = Vec::new();
    for primary in known_primaries.iter().take(MAX_KNOWN_CLUSTER_PRIMARIES) {
        if !candidates.contains(primary) {
            candidates.push(primary.clone());
        }
    }
    for endpoint in configured {
        if !candidates.contains(endpoint) {
            candidates.push(endpoint.clone());
        }
    }
    candidates
}

/// `redirect_node` yields `host:port` (or `[ipv6]:port`) plus the slot id.
pub(super) fn moved_endpoint(address: &str) -> RedisResult<(String, u16)> {
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:").ok_or_else(unavailable)?;
        (host, port)
    } else {
        address.rsplit_once(':').ok_or_else(unavailable)?
    };
    parse_address(&[host.to_owned(), port.to_owned()])
}

fn parse_address(values: &[String]) -> RedisResult<(String, u16)> {
    if values.len() != 2 {
        return Err(unavailable());
    }
    let host = values[0].trim_matches(['[', ']']);
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '@' | '?' | '#'))
    {
        return Err(unavailable());
    }
    let port = values[1]
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(unavailable)?;
    Ok((host.to_owned(), port))
}
fn is_master(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.first().is_some_and(|role| match role {
            Value::BulkString(bytes) => bytes == b"master",
            Value::SimpleString(s) => s == "master",
            _ => false,
        }),
        _ => false,
    }
}

pub(super) enum RoleObservation {
    Master,
    NotMaster,
    Unknown,
}

pub(super) enum RoleReply {
    Ready(RedisResult<Value>),
    TimedOut,
}

pub(super) fn classify_role(role: RoleReply) -> RoleObservation {
    match role {
        RoleReply::Ready(Ok(value)) if is_master(&value) => RoleObservation::Master,
        RoleReply::Ready(Ok(_)) => RoleObservation::NotMaster,
        RoleReply::Ready(Err(_)) | RoleReply::TimedOut => RoleObservation::Unknown,
    }
}

pub(super) fn sentinel_probe_coverage(ping_live: usize, pool_size: usize, role: RoleObservation) -> (usize, usize) {
    match role {
        RoleObservation::NotMaster => (0, pool_size),
        RoleObservation::Master | RoleObservation::Unknown => (ping_live, pool_size),
    }
}

/// Cluster `route_command` returns NoRetry server errors as values. Callers
/// match `Err` with `code() == Some("NOSCRIPT")` to fall back to EVAL.
pub(super) fn normalize_command_value(result: RedisResult<Value>) -> RedisResult<Value> {
    match result {
        Ok(value) => value.extract_error(),
        Err(error) => Err(error),
    }
}

/// A late I/O or READONLY result may schedule recovery only for the snapshot
/// that is still published. The replacement generation keeps its own flag.
pub(super) fn command_failure_requests_recovery(snapshot_current: bool, error: &redis::RedisError) -> bool {
    snapshot_current && (error.is_io_error() || error.code() == Some("READONLY"))
}

pub(super) fn cluster_ping_succeeded(result: Result<RedisResult<Value>, tokio::time::error::Elapsed>) -> bool {
    match result {
        Ok(value) => normalize_command_value(value).is_ok_and(|value| is_pong(&value)),
        Err(_) => false,
    }
}

fn is_pong(value: &Value) -> bool {
    match value {
        Value::SimpleString(text) => text.eq_ignore_ascii_case("pong"),
        Value::BulkString(bytes) => bytes.eq_ignore_ascii_case(b"pong"),
        _ => false,
    }
}

/// Read CLUSTER SLOTS from a node that already belongs to this connection.
/// Wildcard and empty hostnames then use that same node, matching its slot-map key.
async fn primaries_from_connection(
    connection: &mut redis::cluster_async::ClusterConnection,
    seeds: &[(String, u16)],
) -> RedisResult<Vec<(String, u16)>> {
    let mut last_error = unavailable();
    for (host, port) in seeds {
        let mut slots = redis::cmd("CLUSTER");
        slots.arg("SLOTS");
        let result = connection
            .route_command(
                slots,
                RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress {
                    host: host.clone(),
                    port: *port,
                }),
            )
            .await;
        match normalize_command_value(result) {
            Ok(value) => return parse_primaries(value, host),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Follow only a parsed MOVED target. A malformed redirect is an error.
/// ASK and other results do not produce a target.
pub(super) fn moved_target(result: &RedisResult<Value>) -> RedisResult<Option<(String, u16)>> {
    let Err(error) = result else {
        return Ok(None);
    };
    if !matches!(error.retry_method(), redis::RetryMethod::MovedRedirect) {
        return Ok(None);
    }
    let Some((address, _slot)) = error.redirect_node() else {
        return Err(unavailable());
    };
    // redis-rs returns (host:port, slot), not (host, port).
    Ok(Some(moved_endpoint(address)?))
}

/// A certificate rejection, recognized from the dependency's error text. The
/// text itself is never surfaced; callers map this to a fixed class.
pub(super) fn is_tls_verification_error(error: &redis::RedisError) -> bool {
    let detail = error.to_string().to_ascii_lowercase();
    detail.contains("certificate")
        || detail.contains("webpki")
        || detail.contains("unknownissuer")
        || detail.contains("notvalidforname")
}

/// Keep successful dials and one representative failure: the first TLS
/// verification failure if any, otherwise the first failure. Completion order
/// varies, so preferring the actionable TLS class keeps the result stable. The
/// failure feeds only the fixed initialization classifier, never a log.
pub(super) fn split_dial_results<T>(results: Vec<RedisResult<T>>) -> (Vec<T>, Option<redis::RedisError>) {
    let mut ok = Vec::with_capacity(results.len());
    let mut representative: Option<redis::RedisError> = None;
    for result in results {
        match result {
            Ok(value) => ok.push(value),
            Err(error) => {
                let replace = match &representative {
                    None => true,
                    Some(kept) => !is_tls_verification_error(kept) && is_tls_verification_error(&error),
                };
                if replace {
                    representative = Some(error);
                }
            }
        }
    }
    (ok, representative)
}

const MAX_LEARNED_SENTINELS: usize = 16;

/// A `(host, port)` endpoint as dialed.
type Endpoint = (String, u16);

/// Elect the primary from one discovery round. Votes are deduplicated by the
/// answering Sentinel's run ID before counting, so neither repeated config nor
/// a configured name that is also learned by IP can add votes. Learned peers
/// come only from votes for the winner.
pub(super) fn tally_sentinel_votes(votes: Vec<SentinelVote>) -> Option<(Endpoint, Vec<Endpoint>)> {
    let mut unique: Vec<SentinelVote> = Vec::with_capacity(votes.len());
    for vote in votes {
        if !unique.iter().any(|seen| seen.sentinel_id == vote.sentinel_id) {
            unique.push(vote);
        }
    }
    let addresses: Vec<_> = unique.iter().map(|vote| vote.primary.clone()).collect();
    let primary = choose_sentinel_primary(&addresses)?;
    let winner = endpoint_identity(&primary);
    let peers = unique
        .iter()
        .filter(|vote| endpoint_identity(&vote.primary) == winner)
        .flat_map(|vote| vote.learned.iter().cloned());
    Some((primary, dedup_endpoints(peers, MAX_LEARNED_SENTINELS)))
}

/// Keep the first spelling of each normalized endpoint, up to `limit`.
fn dedup_endpoints(endpoints: impl IntoIterator<Item = (String, u16)>, limit: usize) -> Vec<(String, u16)> {
    let mut seen = Vec::new();
    let mut unique = Vec::new();
    for endpoint in endpoints {
        if unique.len() == limit {
            break;
        }
        let identity = endpoint_identity(&endpoint);
        if !seen.contains(&identity) {
            seen.push(identity);
            unique.push(endpoint);
        }
    }
    unique
}

/// Dial each normalized Sentinel endpoint once and preserve configured seed
/// priority. This only avoids redundant dials; vote uniqueness is enforced by
/// run ID in `tally_sentinel_votes`.
pub(super) fn sentinel_discovery_candidates(
    configured: &[(String, u16)],
    learned: &[(String, u16)],
) -> Vec<(String, u16)> {
    dedup_endpoints(configured.iter().chain(learned).cloned(), usize::MAX)
}

/// DNS names compare case-insensitively. A trailing root dot is kept: with
/// resolver search domains `name` and `name.` can be different hosts. IP
/// literals compare canonically, with IPv4-mapped IPv6 folded to IPv4.
pub(super) fn endpoint_identity((host, port): &(String, u16)) -> (String, u16) {
    let host = host.trim_matches(['[', ']']);
    let host = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_canonical().to_string(),
        Err(_) => host.to_ascii_lowercase(),
    };
    (host, *port)
}

/// Unique plurality by normalized address; a tie publishes nothing. The
/// first spelling seen for the winner is the one dialed.
pub(super) fn choose_sentinel_primary(votes: &[(String, u16)]) -> Option<(String, u16)> {
    let mut counts: Vec<(Endpoint, &Endpoint, usize)> = Vec::new();
    for vote in votes {
        let identity = endpoint_identity(vote);
        if let Some((_, _, count)) = counts.iter_mut().find(|(seen, _, _)| *seen == identity) {
            *count += 1;
        } else {
            counts.push((identity, vote, 1));
        }
    }
    let mut winner: Option<&(String, u16)> = None;
    let mut winner_count = 0;
    let mut tie = false;
    for (_, address, count) in counts {
        if winner.is_none() || count > winner_count {
            winner = Some(address);
            winner_count = count;
            tie = false;
        } else if count == winner_count {
            tie = true;
        }
    }
    if tie {
        None
    } else {
        winner.cloned()
    }
}

fn learned_sentinels(peers: Value) -> Vec<(String, u16)> {
    let Value::Array(peers) = peers else {
        return Vec::new();
    };
    let mut learned = Vec::new();
    for peer in peers {
        let Ok(fields) = redis::from_redis_value::<std::collections::HashMap<String, String>>(peer) else {
            continue;
        };
        if let (Some(host), Some(port)) = (fields.get("ip"), fields.get("port")) {
            if let Ok(address) = parse_address(&[host.clone(), port.clone()]) {
                learned.push(address);
            }
        }
    }
    dedup_endpoints(learned, MAX_LEARNED_SENTINELS)
}

/// A hostname of `?` is an unknown Cluster node. Skip that node and keep going.
/// Do not substitute the answering node, and do not accept `?` in `parse_address`.
fn slot_node_address(node: &Value, responder: &str) -> RedisResult<Option<(String, u16)>> {
    let Value::Array(fields) = node else {
        return Err(unavailable());
    };
    if fields.len() < 2 {
        return Err(unavailable());
    }
    let host = match &fields[0] {
        Value::Nil => responder.to_owned(),
        value => {
            let host: String = redis::from_redis_value(value.clone())?;
            if host == "?" {
                return Ok(None);
            }
            if host.is_empty() || host == "0.0.0.0" || host == "::" {
                responder.to_owned()
            } else {
                host
            }
        }
    };
    let port: u16 = redis::from_redis_value(fields[1].clone())?;
    Ok(Some(parse_address(&[host, port.to_string()])?))
}

fn parse_primaries(value: Value, responder: &str) -> RedisResult<Vec<(String, u16)>> {
    let Value::Array(ranges) = value else {
        return Err(unavailable());
    };
    if ranges.len() > 16384 {
        return Err(unavailable());
    }
    let mut slots = vec![false; 16384];
    let mut primaries = Vec::new();
    for range in ranges {
        let Value::Array(fields) = range else {
            return Err(unavailable());
        };
        let (Some(Value::Int(start)), Some(Value::Int(end))) = (fields.first(), fields.get(1)) else {
            return Err(unavailable());
        };
        if *start < 0 || end < start || *end >= 16384 {
            return Err(unavailable());
        }
        let mut address = None;
        for node in fields.iter().skip(2) {
            if let Some(candidate) = slot_node_address(node, responder)? {
                address = Some(candidate);
                break;
            }
        }
        let Some(address) = address else {
            continue;
        };
        for slot in &mut slots[*start as usize..=*end as usize] {
            if *slot {
                return Err(unavailable());
            }
            *slot = true;
        }
        if !primaries.contains(&address) {
            primaries.push(address);
        }
    }
    if primaries.is_empty() {
        return Err(unavailable());
    }
    Ok(primaries)
}

/// Route every business operation to exactly one node. In particular, DEL must
/// never use redis-rs fanout: a shard MOVED cannot authorize replaying other shards.
fn command_route(cmd: &Cmd) -> RedisResult<RoutingInfo> {
    use redis::cluster_routing::{Route, SlotAddr};
    let Some(redis::Arg::Simple(name)) = cmd.args_iter().next() else {
        return Err(unavailable());
    };
    let key_index = match name {
        b"PING" => return Ok(RoutingInfo::SingleNode(SingleNodeRoutingInfo::RandomPrimary)),
        b"EVAL" | b"EVALSHA" => 3,
        b"GET" | b"SET" | b"DEL" | b"EXISTS" | b"EXPIRE" | b"TTL" | b"INCR" | b"INCRBY" | b"HGET" | b"HSET"
        | b"HDEL" | b"HGETALL" | b"HEXISTS" | b"RPUSH" | b"LPOP" | b"LLEN" => 1,
        _ => return Err(unavailable()),
    };
    let Some(redis::Arg::Simple(key)) = cmd.args_iter().nth(key_index) else {
        return Err(unavailable());
    };
    Ok(RoutingInfo::SingleNode(SingleNodeRoutingInfo::SpecificNode(
        Route::with_key(key, SlotAddr::Master),
    )))
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/backend_test.rs"]
mod tests;
