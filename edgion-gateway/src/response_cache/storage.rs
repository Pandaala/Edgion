//! Canonical count LRU plus independent capacity and pending-owner accounting.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use edgion_algo::cache::ShardedLruCache;
use parking_lot::Mutex;
use pingora_http::ResponseHeader;

use super::policy::{self, Freshness, MAX_BODY, MAX_METADATA};

pub(super) type Key = [u8; 32];
const ENTRY_CONTROL: usize = 1024;
const READER_CONTROL: usize = 1024;
const HEADER_WORK: usize = MAX_METADATA * 2 + policy::MAX_FIELDS * 512;

static STORE: OnceLock<Arc<Store>> = OnceLock::new();
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(crate) fn epoch_control_bytes() -> usize {
    std::mem::size_of::<Epoch>() + 16
}

#[derive(Clone, Default)]
pub(crate) struct PublicationEpoch {
    pub(crate) id: u64,
    pub(super) guard: Option<Arc<Epoch>>,
}

impl PublicationEpoch {
    pub(crate) fn is_initialized(&self) -> bool {
        self.guard.is_some()
    }
}

pub(crate) fn current_epoch() -> PublicationEpoch {
    STORE
        .get()
        .map_or_else(PublicationEpoch::default, |store| PublicationEpoch {
            id: 0,
            guard: Some(Arc::clone(&store.fence.lock())),
        })
}

pub(crate) fn advance_epoch() -> PublicationEpoch {
    let Some(store) = STORE.get() else {
        return PublicationEpoch::default();
    };
    store.advance_epoch()
}

pub(crate) fn initialize(limits: &crate::cli::config::ResponseCacheLimits) -> Result<(), &'static str> {
    if STORE.get().is_some() {
        return Err("response_cache was already initialized");
    }
    let store = Store::from_limits(limits)?;
    STORE.set(store).map_err(|_| "response_cache was already initialized")
}

pub(crate) fn process_store() -> Option<&'static Arc<Store>> {
    STORE.get()
}

pub(crate) fn nominate(
    ctx: &mut crate::EdgionHttpContext,
    policy: &Arc<crate::plugins::http::response_cache::ResponseCachePolicy>,
    store: &Arc<Store>,
) -> crate::plugins::runtime::traits::session::ResponseCacheIntentAdmission {
    use crate::plugins::runtime::traits::session::ResponseCacheIntentAdmission as Admission;
    if !ctx.request_info.in_request_body_stage || ctx.response_cache_bypassed {
        return Admission::Bypassed;
    }
    if let Some(state) = &ctx.response_cache {
        if state.intent.as_ref().is_some_and(|current| current.equivalent(policy)) {
            return Admission::AlreadyAdmitted;
        }
        ctx.response_cache = None;
        ctx.response_cache_bypassed = true;
        return Admission::Bypassed;
    }
    let Some(token) = ctx.listener_publication().map(|view| &view.cache_epoch) else {
        return Admission::Bypassed;
    };
    let Some(epoch) = token
        .guard
        .as_ref()
        .filter(|epoch| token.id != 0 && epoch.active.load(Ordering::Acquire))
    else {
        return Admission::Bypassed;
    };
    let Some(lease) = Lease::acquire(&store.ledger, std::mem::size_of::<RequestState>() + 16, false) else {
        return Admission::Bypassed;
    };
    let state = Box::new(RequestState {
        store: Arc::clone(store),
        epoch: Arc::clone(epoch),
        key: [0; 32],
        key_ready: false,
        header_certificate: None,
        intent: Some(Arc::clone(policy)),
        max_ttl: policy.max_ttl,
        expected_origin: None,
        expected_authority: None,
        actual_authority: None,
        actual_discovery: None,
        actual_traffic: None,
        actual_tls: None,
        origin_invalid: false,
        pending: None,
        delivered_header_lease: None,
        _lease: lease,
    });
    ctx.response_cache = Some(state);
    Admission::Admitted
}

#[derive(Default, serde::Serialize)]
pub(crate) struct Stats {
    pub infrastructure_bytes: usize,
    pub allocated_bytes: usize,
    pub pending_bytes: usize,
    pub entries: usize,
    pub reader_pins: usize,
}

pub(crate) fn snapshot_stats() -> Stats {
    STORE.get().map_or_else(Stats::default, |store| Stats {
        infrastructure_bytes: store.infrastructure,
        allocated_bytes: store.ledger.total.load(Ordering::Acquire),
        pending_bytes: store.ledger.pending.load(Ordering::Acquire),
        entries: store.entries.len(),
        reader_pins: store.ledger.readers.load(Ordering::Acquire),
    })
}

struct Ledger {
    total: AtomicUsize,
    pending: AtomicUsize,
    readers: AtomicUsize,
    limit: usize,
    pending_limit: usize,
}

fn reserve(counter: &AtomicUsize, amount: usize, limit: usize) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(amount).filter(|next| *next <= limit)
        })
        .is_ok()
}

pub(super) struct Lease {
    ledger: Arc<Ledger>,
    amount: usize,
    pending: bool,
}

impl Lease {
    fn acquire(ledger: &Arc<Ledger>, amount: usize, pending: bool) -> Option<Self> {
        if !reserve(&ledger.total, amount, ledger.limit) {
            return None;
        }
        if pending && !reserve(&ledger.pending, amount, ledger.pending_limit) {
            ledger.total.fetch_sub(amount, Ordering::AcqRel);
            return None;
        }
        Some(Self {
            ledger: Arc::clone(ledger),
            amount,
            pending,
        })
    }

    fn publish(&mut self) {
        if self.pending {
            self.ledger.pending.fetch_sub(self.amount, Ordering::AcqRel);
            self.pending = false;
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.pending {
            self.ledger.pending.fetch_sub(self.amount, Ordering::AcqRel);
        }
        self.ledger.total.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

pub(super) struct Epoch {
    active: AtomicBool,
    _lease: Lease,
}

impl Epoch {
    fn admit(ledger: &Arc<Ledger>, active: bool) -> Option<Arc<Self>> {
        let lease = Lease::acquire(ledger, epoch_control_bytes(), false)?;
        Some(Arc::new(Self {
            active: AtomicBool::new(active),
            _lease: lease,
        }))
    }
}

pub(crate) struct Store {
    entries: ShardedLruCache<Key, Arc<Entry>>,
    fence: Mutex<Arc<Epoch>>,
    ledger: Arc<Ledger>,
    infrastructure: usize,
    body_cap: usize,
    metadata_cap: usize,
}

impl Store {
    fn from_limits(limits: &crate::cli::config::ResponseCacheLimits) -> Result<Arc<Self>, &'static str> {
        let infrastructure = limits.validate()?;
        // Fixed infrastructure reserves the canonical index's worst-case
        // capacity before any containers or epoch control owner are allocated.
        let ledger = Arc::new(Ledger {
            total: AtomicUsize::new(infrastructure - epoch_control_bytes()),
            pending: AtomicUsize::new(0),
            readers: AtomicUsize::new(0),
            limit: limits.total_bytes,
            pending_limit: limits.pending_bytes,
        });
        let initial = Epoch::admit(&ledger, false).ok_or("response_cache initial epoch admission failed")?;
        Ok(Arc::new(Self {
            entries: ShardedLruCache::new(limits.max_entries, limits.shards),
            fence: Mutex::new(initial),
            ledger,
            infrastructure,
            body_cap: limits.max_body_bytes,
            metadata_cap: limits.max_metadata_bytes,
        }))
    }

    fn advance_epoch(&self) -> PublicationEpoch {
        let id = NEXT_EPOCH
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| old.checked_add(1))
            .ok()
            .map(|old| old + 1);
        // Admission precedes Arc allocation and the retirement fence. Refusal
        // still retires/clears, retaining the one inactive standing owner.
        let candidate = id.and_then(|_| Epoch::admit(&self.ledger, true));
        let mut holder = self.fence.lock();
        holder.active.store(false, Ordering::Release);
        self.entries.clear();
        if let Some(candidate) = candidate {
            *holder = candidate;
        }
        PublicationEpoch {
            id: id.unwrap_or(0),
            guard: Some(Arc::clone(&holder)),
        }
    }

    pub(super) fn lookup(&self, key: &Key, epoch: &Epoch) -> Option<Hit> {
        if !epoch.active.load(Ordering::Acquire) {
            return None;
        }
        let entry = self.entries.get(key)?;
        if entry.freshness.remaining(Instant::now()).is_none() {
            self.remove_if_current(key, &entry);
            return None;
        }
        let headers = Lease::acquire(&self.ledger, HEADER_WORK, false)?;
        let reader = Lease::acquire(&self.ledger, READER_CONTROL, false)?;
        let Some(mut header) = entry.decode() else {
            self.remove_if_current(key, &entry);
            return None;
        };
        header.remove_header("transfer-encoding");
        header.insert_header("content-length", entry.used.to_string()).ok()?;
        self.ledger.readers.fetch_add(1, Ordering::AcqRel);
        let body = Bytes::from_owner(Reader {
            entry: Arc::clone(&entry),
            _lease: reader,
        });
        Some(Hit {
            header,
            body,
            entry,
            header_lease: Some(headers),
        })
    }

    fn remove_if_current(&self, key: &Key, expected: &Arc<Entry>) {
        let _guard = self.fence.lock();
        if self
            .entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(&current, expected))
        {
            self.entries.remove(key);
        }
    }
}

pub(super) struct Entry {
    arena: Vec<u8>,
    metadata: Vec<u8>,
    used: usize,
    pub(super) freshness: Freshness,
    vary: super::key::VaryCertificate,
    _lease: Lease,
}

impl Entry {
    fn decode(&self) -> Option<ResponseHeader> {
        if self.used > self.arena.len() || self.arena.len() > MAX_BODY || self.metadata.len() > MAX_METADATA {
            return None;
        }
        let mut header = ResponseHeader::build(200, Some(policy::MAX_FIELDS)).ok()?;
        for (index, field) in fields(&self.metadata).enumerate() {
            if index >= policy::MAX_FIELDS {
                return None;
            }
            let (name, value) = field.ok()?;
            header
                .append_header(http::HeaderName::from_bytes(name).ok()?, value)
                .ok()?;
        }
        if !self.vary.matches(&header.headers) {
            return None;
        }
        // Stored fields must still form the raw public-origin contract. A
        // syntactically valid HeaderMap alone does not establish integrity.
        Freshness::from_headers(
            &header,
            self.freshness.header_at,
            self.freshness.header_at,
            SystemTime::now(),
            self.freshness.lifetime,
        )?;
        Some(header)
    }

    pub(super) fn matches_contract(&self, header: &ResponseHeader) -> bool {
        fields(&self.metadata).all(|field| field.is_ok())
            && policy::supported_headers(header)
            && ["cache-control", "date", "age"].iter().all(|name| {
                let mut actual = header.headers.get_all(*name).iter();
                let first = actual.next().map(|value| value.as_bytes());
                actual.next().is_none()
                    && first
                        == fields(&self.metadata)
                            .filter_map(Result::ok)
                            .find(|(key, _)| key.eq_ignore_ascii_case(name.as_bytes()))
                            .map(|(_, v)| v)
            })
    }
    fn matches_final_transport_contract(&self, header: &ResponseHeader) -> bool {
        let mut dates = header.headers.get_all("date").iter();
        let valid_date = dates.next().is_some_and(|date| {
            date.to_str()
                .ok()
                .and_then(|date| httpdate::parse_http_date(date).ok())
                .is_some()
        }) && dates.next().is_none();
        valid_date
            && fields(&self.metadata).all(|field| field.is_ok())
            && policy::supported_headers(header)
            && ["cache-control", "age"].iter().all(|name| {
                let mut actual = header.headers.get_all(*name).iter();
                let first = actual.next().map(|value| value.as_bytes());
                actual.next().is_none()
                    && first
                        == fields(&self.metadata)
                            .filter_map(Result::ok)
                            .find(|(key, _)| key.eq_ignore_ascii_case(name.as_bytes()))
                            .map(|(_, value)| value)
            })
    }
}

fn fields(metadata: &[u8]) -> impl Iterator<Item = Result<(&[u8], &[u8]), ()>> {
    let mut remaining = metadata;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let field = (|| {
            let lengths = remaining.get(..6)?;
            let name_len = usize::from(u16::from_be_bytes([lengths[0], lengths[1]]));
            let value_len = u32::from_be_bytes([lengths[2], lengths[3], lengths[4], lengths[5]]) as usize;
            let name_end = 6usize.checked_add(name_len)?;
            let end = name_end.checked_add(value_len)?;
            let name = remaining.get(6..name_end)?;
            let value = remaining.get(name_end..end)?;
            remaining = remaining.get(end..)?;
            Some((name, value))
        })();
        if field.is_none() {
            remaining = &[];
        }
        Some(field.ok_or(()))
    })
}

struct Reader {
    entry: Arc<Entry>,
    _lease: Lease,
}

impl AsRef<[u8]> for Reader {
    fn as_ref(&self) -> &[u8] {
        &self.entry.arena[..self.entry.used]
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self._lease.ledger.readers.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct Hit {
    pub(crate) header: ResponseHeader,
    pub(crate) body: Bytes,
    pub(super) entry: Arc<Entry>,
    header_lease: Option<Lease>,
}

pub(super) struct Pending {
    entry: Entry,
    complete: bool,
    pretransport_sealed: bool,
}

impl Pending {
    fn create(
        store: &Store,
        header: &ResponseHeader,
        request_at: Instant,
        max_ttl: Duration,
        certificate: Option<&super::key::HeaderCertificate>,
    ) -> Option<Self> {
        let vary = super::key::VaryCertificate::parse(&header.headers)?;
        if certificate.map_or(!vary.is_empty(), |certificate| !certificate.permits(&vary)) {
            return None;
        }
        let now = Instant::now();
        let freshness = Freshness::from_headers(header, request_at, now, SystemTime::now(), max_ttl)?;
        freshness.remaining(now)?;
        let mut size = 0usize;
        for (name, value) in &header.headers {
            size = size
                .checked_add(6)?
                .checked_add(name.as_str().len())?
                .checked_add(value.len())?;
        }
        if store.body_cap > MAX_BODY || size > store.metadata_cap || header.headers.contains_key("connection") {
            return None;
        }
        let amount = store
            .body_cap
            .checked_add(store.metadata_cap)?
            .checked_add(ENTRY_CONTROL)?;
        let lease = Lease::acquire(&store.ledger, amount, true)?;
        let mut arena = Vec::new();
        arena.try_reserve_exact(store.body_cap).ok()?;
        if arena.capacity() != store.body_cap {
            return None;
        }
        arena.resize(store.body_cap, 0);
        let mut metadata = Vec::new();
        metadata.try_reserve_exact(store.metadata_cap).ok()?;
        if metadata.capacity() != store.metadata_cap {
            return None;
        }
        for (name, value) in &header.headers {
            metadata.extend_from_slice(&u16::try_from(name.as_str().len()).ok()?.to_be_bytes());
            metadata.extend_from_slice(&u32::try_from(value.len()).ok()?.to_be_bytes());
            metadata.extend_from_slice(name.as_str().as_bytes());
            metadata.extend_from_slice(value.as_bytes());
        }
        Some(Self {
            entry: Entry {
                arena,
                metadata,
                used: 0,
                freshness,
                vary,
                _lease: lease,
            },
            complete: false,
            pretransport_sealed: false,
        })
    }

    /// Copy only; all backing capacity was admitted before body callbacks started.
    fn push(&mut self, bytes: Option<&Bytes>, eos: bool) -> bool {
        let bytes = bytes.map_or(&[][..], Bytes::as_ref);
        let Some(end) = self
            .entry
            .used
            .checked_add(bytes.len())
            .filter(|end| *end <= self.entry.arena.len())
        else {
            return false;
        };
        if self.complete {
            return false;
        }
        self.entry.arena[self.entry.used..end].copy_from_slice(bytes);
        self.entry.used = end;
        self.complete = eos;
        true
    }
}

pub(crate) struct RequestState {
    store: Arc<Store>,
    epoch: Arc<Epoch>,
    key: Key,
    max_ttl: Duration,
    intent: Option<Arc<crate::plugins::http::response_cache::ResponseCachePolicy>>,
    key_ready: bool,
    header_certificate: Option<super::key::HeaderCertificate>,
    expected_origin: Option<super::key::OriginStamp>,
    expected_authority: Option<[u8; 32]>,
    actual_authority: Option<[u8; 32]>,
    actual_discovery: Option<super::key::DiscoveryStamp>,
    actual_traffic: Option<super::key::TrafficStamp>,
    actual_tls: Option<super::key::TlsStamp>,
    origin_invalid: bool,
    pending: Option<Pending>,
    // Once decode transfers to the Session, retain its control charge for this context's lifetime.
    delivered_header_lease: Option<Lease>,
    _lease: Lease,
}

impl RequestState {
    pub(super) async fn bind(
        &mut self,
        session: &mut pingora_proxy::Session,
        ctx: &mut crate::EdgionHttpContext,
    ) -> Option<()> {
        use super::key::{Builder, OriginStamp, TlsResult, WORK_LIMIT};
        use crate::backends::discovery::get_service_discovery_coordinator;
        use crate::plugins::runtime::{PingoraSessionAdapter, PluginSession};
        // C0 external fixtures construct a bound state directly. Production
        // intents reach this one tail binding only after successful plugins.
        if self.key_ready {
            return Some(());
        }
        if !super::policy::request_is_eligible(&session.req_header().headers) {
            return None;
        }
        let policy = Arc::clone(self.intent.as_ref()?);
        if !self.epoch.active.load(Ordering::Acquire) || self.origin_invalid {
            return None;
        }
        let _work = Lease::acquire(&self.store.ledger, WORK_LIMIT, false)?;
        let epoch_id = ctx.listener_publication()?.cache_epoch.id;
        if epoch_id == 0 || !ctx.plugin_scope()?.scope.available {
            return None;
        }
        let route = Arc::clone(ctx.route_unit.as_ref()?);
        if route.backends.len() != 1 {
            return None;
        }
        let backend = &route.backends[0];
        if backend.weight.unwrap_or(1) <= 0 || backend.filters.as_ref().is_some_and(|filters| !filters.is_empty()) {
            return None;
        }
        let service = backend.permitted_service()?;
        let generation = get_service_discovery_coordinator().load_generation();
        let available = generation.available(service.key())?;
        if available
            .raw
            .service
            .spec
            .as_ref()
            .is_some_and(|spec| spec.type_.as_deref() == Some("ExternalName"))
        {
            return None;
        }
        let section = available.resolve_section(service.section()).ok()?;
        if section.ready_slots.is_empty() {
            return None;
        }
        let discovery = super::key::discovery_stamp(service, available, &section)?;
        let (tls_lookup, tls) = crate::backends::policy::query_backend_tls_policy_for_service_report(service);
        if tls.discovery != Some(discovery) || tls_lookup.is_invalid() {
            return None;
        }
        let traffic_snapshot = crate::backends::policy::get_backend_traffic_policy_store().snapshot();
        let effective = traffic_snapshot.get_effective(service.policy_target());
        if !crate::backends::policy::ancestor_admitted(
            &tls_lookup,
            effective.as_deref(),
            ctx.gateway_info.as_deref().map(|gateway| gateway.gateway_key()),
        ) {
            return None;
        }
        let traffic = super::key::traffic_stamp(traffic_snapshot.generation(), effective.as_deref())?;
        let authority = if let Some(compiled) = effective.as_ref().and_then(|policy| policy.upstream_authority.as_ref())
        {
            if !super::key::authority_is_eligible(compiled) {
                return None;
            }
            let adapter = PingoraSessionAdapter::new(session, ctx);
            Some(crate::backends::policy::backend_traffic::authority::render_authority(compiled, &adapter).ok()?)
        } else {
            None
        };
        let expected_authority = super::key::authority_stamp(authority.as_deref())?;
        let global = ctx.resolved_global_plugins();
        let gateway = ctx.resolved_gateway_plugins();
        let refs = ctx.resolved_route_ext_refs();
        if route.plugin_runtime.ext_ref_keys().len() != refs.as_ref().map_or(0, |refs| refs.len()) {
            return None;
        }
        let mut key = Builder::new();
        key.number(u64::from(authority.is_some()))?;
        if let Some(authority) = authority.as_ref() {
            key.frame(authority.as_bytes())?;
        }
        key.number(epoch_id)?;
        key.frame(route.resource_key.as_bytes())?;
        key.number(u64::try_from(route.matched_info.rule_id).ok()?)?;
        for group in [global.as_deref(), gateway.as_deref()] {
            key.number(group.map_or(0, |group| group.len()) as u64)?;
            for runtime in group.into_iter().flatten() {
                if !runtime.cache_safe() {
                    return None;
                }
                for header in &runtime.cache_capabilities().credential_headers {
                    if session.req_header().headers.contains_key(header) {
                        return None;
                    }
                }
                key.number(runtime.cache_identity())?;
            }
        }
        if !route.plugin_runtime.cache_safe() {
            return None;
        }
        for header in &route.plugin_runtime.cache_capabilities().credential_headers {
            if session.req_header().headers.contains_key(header) {
                return None;
            }
        }
        key.number(route.plugin_runtime.cache_identity())?;
        key.number(refs.as_ref().map_or(0, |refs| refs.len()) as u64)?;
        for runtime in refs.as_deref().into_iter().flatten() {
            let runtime = runtime.as_ref()?;
            if !runtime.cache_safe() {
                return None;
            }
            for header in &runtime.cache_capabilities().credential_headers {
                if session.req_header().headers.contains_key(header) {
                    return None;
                }
            }
            key.number(runtime.cache_identity())?;
        }
        if !session.as_downstream_mut().is_body_done() {
            return None;
        }
        let header = session.req_header();
        key.frame(header.method.as_str().as_bytes())?;
        key.frame(ctx.path().as_bytes())?;
        key.frame(ctx.query(header).unwrap_or("").as_bytes())?;
        let mut hosts = header.headers.get_all("host").iter();
        let host = hosts.next();
        if hosts.next().is_some() {
            return None;
        }
        if let Some(host) = host {
            if host.len() > super::key::MATERIAL_LIMIT || host.to_str().ok()?.parse::<http::uri::Authority>().is_err() {
                return None;
            }
        }
        key.frame(host.map_or(b"".as_slice(), |host| host.as_bytes()))?;
        key.frame(
            header
                .uri
                .authority()
                .map_or("", |authority| authority.as_str())
                .as_bytes(),
        )?;
        key.frame(ctx.request_info.remote_ip.as_bytes())?;
        key.frame(ctx.request_info.downstream_peer_ip.as_bytes())?;
        key.frame(ctx.request_info.x_forwarded_for.as_deref().unwrap_or("").as_bytes())?;
        key.frame(ctx.remote_ip_header.as_str().as_bytes())?;
        let header_certificate = key.headers(
            &header.headers,
            &ctx.request_info.trace_id,
            edgion_common::test_mode::is_integration_testing_mode(),
        )?;
        // The bounded header-name owner lives in its already admitted Box,
        // never in an ordinary request's suspended async future.
        self.header_certificate = Some(header_certificate);
        match header.method.as_str() {
            "GET" => {
                if ctx.request_info.body_declared != Some(false)
                    || ctx.request_body.cached_len().ok().flatten().unwrap_or(0) != 0
                {
                    return None;
                }
                key.frame(b"bodyless")?;
            }
            "POST" if policy.graphql_queries => {
                let proof = *ctx.response_cache_graphql_proof.get()?;
                if proof.operation != edgion_resources::graphql_request::GraphqlOperationType::Query
                    || ctx.request_body.generation() != Some(proof.generation)
                    || ctx.request_body.cached_len().ok()?? > 65536
                {
                    return None;
                }
                let scan = ctx.request_body.sha256_scan(65536).await.ok()?;
                if ctx.request_body.generation() != Some(proof.generation) {
                    return None;
                }
                key.number(scan.bytes_scanned)?;
                key.frame(&scan.value)?;
                key.frame(&proof.name_digest)?;
            }
            _ => return None,
        }
        key.frame(service.namespace().as_bytes())?;
        key.frame(service.name().as_bytes())?;
        key.frame(&discovery.service)?;
        key.frame(&discovery.section)?;
        key.frame(&discovery.fingerprint)?;
        key.number(discovery.membership)?;
        key.number(traffic.generation)?;
        key.number(u64::from(traffic.result.is_some()))?;
        if let Some(result) = traffic.result {
            key.frame(&result)?;
        }
        key.number(tls.generation)?;
        match tls.result {
            TlsResult::Absent => key.frame(b"absent")?,
            TlsResult::Invalid => return None,
            TlsResult::Effective(profile) => {
                key.frame(b"effective")?;
                key.frame(&profile)?;
            }
        }
        for source in &policy.vary {
            let edgion_resources::common::KeyGet::Header { name } = source else {
                return None;
            };
            if !super::key::variation_is_representable(
                &session.req_header().headers,
                &ctx.request_info.trace_id,
                edgion_common::test_mode::is_integration_testing_mode(),
                name,
            ) {
                return None;
            }
            key.frame(name.as_bytes())?;
            self.header_certificate.as_mut()?.add(name)?;
            let adapter = PingoraSessionAdapter::new(session, ctx);
            let value = adapter.key_get(source);
            key.number(u64::from(value.is_some()))?;
            key.frame(value.as_deref().unwrap_or("").as_bytes())?;
        }
        self.key = key.finish();
        self.expected_origin = Some(OriginStamp {
            discovery,
            traffic,
            tls,
        });
        self.expected_authority = Some(expected_authority);
        self.key_ready = true;
        Some(())
    }

    fn origin_matches(&self) -> bool {
        if self.intent.is_none() {
            return true;
        }
        !self.origin_invalid
            && self.expected_authority.is_some()
            && self.actual_authority == self.expected_authority
            && self.expected_origin.is_some_and(|expected| {
                self.actual_discovery == Some(expected.discovery)
                    && self.actual_traffic == Some(expected.traffic)
                    && self.actual_tls == Some(expected.tls)
            })
    }

    pub(crate) fn observe_authority(&mut self, result: Result<Option<&str>, ()>) {
        let stamp = result.ok().and_then(super::key::authority_stamp);
        if stamp.is_none()
            || self.actual_authority.is_some_and(|old| Some(old) != stamp)
            || self.expected_authority.is_some_and(|expected| Some(expected) != stamp)
        {
            self.origin_invalid = true;
            self.pending = None;
        }
        self.actual_authority = stamp;
    }

    pub(super) fn record_discovery(&mut self, stamp: Option<super::key::DiscoveryStamp>) {
        if stamp.is_none()
            || self.actual_discovery.is_some_and(|old| Some(old) != stamp)
            || self
                .expected_origin
                .is_some_and(|expected| Some(expected.discovery) != stamp)
        {
            self.origin_invalid = true;
            self.abandon();
        }
        self.actual_discovery = stamp;
    }

    pub(super) fn record_traffic(&mut self, stamp: Option<super::key::TrafficStamp>) {
        if stamp.is_none()
            || self.actual_traffic.is_some_and(|old| Some(old) != stamp)
            || self
                .expected_origin
                .is_some_and(|expected| Some(expected.traffic) != stamp)
        {
            self.origin_invalid = true;
            self.abandon();
        }
        self.actual_traffic = stamp;
    }

    pub(super) fn record_tls(&mut self, stamp: super::key::TlsStamp) {
        if self.actual_tls.is_some_and(|old| old != stamp)
            || self.expected_origin.is_some_and(|expected| expected.tls != stamp)
        {
            self.origin_invalid = true;
            self.abandon();
        }
        self.actual_tls = Some(stamp);
    }

    pub(super) fn lookup(&self) -> Option<Hit> {
        self.key_ready
            .then(|| self.store.lookup(&self.key, &self.epoch))
            .flatten()
    }

    pub(super) fn capture(&mut self, header: &ResponseHeader, request_at: Instant) {
        self.pending = None;
        if self.key_ready && self.origin_matches() && self.epoch.active.load(Ordering::Acquire) {
            self.pending = Pending::create(
                &self.store,
                header,
                request_at,
                self.max_ttl,
                self.header_certificate.as_ref(),
            );
        }
    }

    pub(super) fn push(&mut self, bytes: Option<&Bytes>, eos: bool) {
        if self.pending.as_mut().is_some_and(|pending| !pending.push(bytes, eos)) {
            self.pending = None;
        }
    }

    pub(super) fn abandon(&mut self) {
        self.pending = None;
    }

    fn permitted_vary(&self, header: &ResponseHeader) -> bool {
        super::key::VaryCertificate::parse(&header.headers).is_some_and(|vary| {
            self.header_certificate
                .as_ref()
                .map_or(vary.is_empty(), |certificate| certificate.permits(&vary))
        })
    }

    pub(super) fn retain_header_charge(&mut self, hit: &mut Hit) {
        if !hit.entry.matches_contract(&hit.header) || !self.permitted_vary(&hit.header) {
            self.store.remove_if_current(&self.key, &hit.entry);
        }
        self.delivered_header_lease = hit.header_lease.take();
    }

    pub(super) fn seal_pending_headers(&mut self, chosen_header: &ResponseHeader) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        pending.pretransport_sealed = false;
        if !pending.entry.matches_contract(chosen_header) || !self.permitted_vary(chosen_header) {
            self.abandon();
            return false;
        }
        if let Some(pending) = self.pending.as_mut() {
            pending.pretransport_sealed = true;
            true
        } else {
            false
        }
    }

    pub(super) fn prepare_publication(&mut self, final_header: Option<&ResponseHeader>) -> bool {
        if self.pending.as_ref().is_none_or(|pending| {
            !pending.complete
                || !pending.pretransport_sealed
                || final_header.is_none_or(|header| {
                    !pending.entry.matches_final_transport_contract(header) || !self.permitted_vary(header)
                })
        }) || !self.origin_matches()
            || !self.epoch.active.load(Ordering::Acquire)
        {
            self.abandon();
            return false;
        }
        true
    }

    pub(super) fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    pub(super) fn publish_pending(&mut self, mut pending: Pending) {
        let _guard = self.store.fence.lock();
        if !pending.complete || !self.epoch.active.load(Ordering::Acquire) {
            return;
        }
        let Some(ttl) = pending.entry.freshness.remaining(Instant::now()) else {
            return;
        };
        pending.entry._lease.publish();
        self.store.entries.insert(self.key, Arc::new(pending.entry), Some(ttl));
    }
}

#[cfg(test)]
#[path = "../../tests/unit/response_cache/storage_test.rs"]
mod tests;
