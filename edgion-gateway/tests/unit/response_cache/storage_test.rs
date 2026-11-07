use super::policy::MAX_FIELDS;
use super::*;
use crate::response_cache::tests::{fixture, response};

#[test]
fn capture_and_reader_slices_allocate_nothing_and_charge_unused_capacity() {
    let (store, epoch, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    let backing = Bytes::from(vec![b'x'; 1024 * 1024]);
    let tiny = backing.slice(300..303);
    let pointer = state.pending.as_ref().unwrap().entry.arena.as_ptr();
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        state.push(Some(&tiny), false);
        state.push(Some(&tiny), true);
    });
    assert_eq!(stats.calls, 0);
    let pending = state.pending.as_ref().unwrap();
    assert_eq!(pending.entry.arena.as_ptr(), pointer);
    assert_eq!(pending.entry.arena.capacity(), 1024);
    assert_eq!(pending.entry.arena.len(), 6);
    assert_eq!(store.stats().1, 1024 + MAX_METADATA + ENTRY_CONTROL);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    assert_eq!(store.stats().1, 0);
    let hit = state.lookup().unwrap();
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
        for _ in 0..500 {
            assert_eq!(&hit.body.slice(0..3)[..], b"xxx");
        }
    });
    assert_eq!(allocations, 0);
    store.retire(&epoch);
    assert_eq!(store.stats().3, 0);
    assert_eq!(store.stats().2, 1);
    let pinned = hit.body.slice(0..1);
    drop(hit);
    drop(state);
    drop(epoch);
    assert!(
        store.stats().0 > store.stats().4,
        "reader slice retains the full arena charge"
    );
    drop(pinned);
    let (total, pending, readers, entries, baseline) = store.stats();
    assert_eq!((total, pending, readers, entries), (baseline, 0, 0, 0));
}

/// Body chunks and the expected published body; `None` means the fill is rejected.
type BodyCase<'a> = (&'a [&'a [u8]], Option<&'a [u8]>);

#[test]
fn arena_starts_empty_and_body_bounds_hold_without_reallocation() {
    let cases: [BodyCase; 4] = [
        (&[], Some(b"")),
        (&[b"abc", b"defgh"], Some(b"abcdefgh")),
        (&[b"abcdefgh", b"i"], None),
        (&[b"abcdefghi"], None),
    ];
    for (chunks, expected) in cases {
        let (store, _epoch, mut state) = fixture(8);
        let header = response();
        state.capture(&header, Instant::now());
        let pending = state.pending.as_ref().unwrap();
        let (pointer, capacity) = (pending.entry.arena.as_ptr(), pending.entry.arena.capacity());
        assert_eq!(pending.entry.arena.len(), 0, "the arena is reserved, not zero-filled");
        assert_eq!(capacity, 8);
        for chunk in chunks {
            state.push(Some(&Bytes::copy_from_slice(chunk)), false);
            if let Some(pending) = state.pending.as_ref() {
                assert_eq!(pending.entry.arena.as_ptr(), pointer);
                assert_eq!(pending.entry.arena.capacity(), capacity);
            }
        }
        state.push(None, true);
        let Some(expected) = expected else {
            assert!(state.pending.is_none(), "overflow past the cap discards the fill");
            assert_eq!(store.stats().1, 0, "the discarded fill releases its lease");
            continue;
        };
        state.push(Some(&Bytes::from_static(b"x")), false);
        assert!(state.pending.is_none(), "push after EOS discards the fill");
        state.capture(&header, Instant::now());
        for chunk in chunks {
            state.push(Some(&Bytes::copy_from_slice(chunk)), false);
        }
        state.push(None, true);
        assert!(state.seal_pending_headers(&header));
        assert!(state.prepare_publication(Some(&header)));
        state.publish();
        let hit = state.lookup().unwrap();
        assert_eq!(&hit.body[..], expected);
        assert_eq!(
            hit.header.headers.get("content-length").unwrap().as_bytes(),
            expected.len().to_string().as_bytes()
        );
        drop(hit);
        drop(state);
        assert_eq!(store.stats().1, 0);
    }
}

#[test]
fn private_eos_overflow_cancellation_and_retirement_never_publish() {
    for mode in 0..4 {
        let (store, epoch, mut state) = fixture(8);
        let header = response();
        state.capture(&header, Instant::now());
        state.push(Some(&Bytes::from_static(b"hello")), false);
        assert_eq!(store.stats().3, 0, "partial writer stays private");
        match mode {
            0 => state.abandon(),
            1 => state.push(Some(&Bytes::from_static(b"overflow")), true),
            2 => {
                state.push(None, true);
                store.retire(&epoch);
                state.publish();
            }
            _ => {
                state.push(None, true);
                let mut denied = header.clone();
                denied.insert_header("set-cookie", "test=1").unwrap();
                assert!(state.seal_pending_headers(&header));
                assert!(!state.prepare_publication(Some(&denied)));
            }
        }
        drop(state);
        drop(epoch);
        let (total, pending, readers, entries, baseline) = store.stats();
        assert_eq!((total, pending, readers, entries), (baseline, 0, 0, 0));
    }
}

/// A sealed fill whose body never saw EOS (for example an upstream stream
/// reset mid-body) is refused by the `complete` guard alone.
#[test]
fn sealed_fill_without_eos_is_refused_at_publication() {
    let (store, epoch, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"hello")), false);
    assert!(state.seal_pending_headers(&header));
    assert!(!state.prepare_publication(Some(&header)));
    assert_eq!(store.stats().3, 0);
    drop(state);
    drop(epoch);
    let (total, pending, readers, entries, baseline) = store.stats();
    assert_eq!((total, pending, readers, entries), (baseline, 0, 0, 0));
}

#[test]
fn pending_or_control_refusal_allocates_no_unadmitted_owner() {
    let store = Store::new(1, 1024, 128 * 1024, 0);
    let epoch = store.epoch();
    let mut state = RequestState::arm(Arc::clone(&store), Arc::clone(&epoch), [1; 32]).unwrap();
    let before = store.stats().0;
    let header = response();
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| state.capture(&header, Instant::now()));
    assert_eq!(allocations, 0);
    assert!(state.pending.is_none());
    assert_eq!(store.stats().0, before);
    let tight = Store::new(1, 8, 8192 + std::mem::size_of::<Epoch>() + 16, 16 * 1024);
    let epoch = tight.epoch();
    let (refused, allocations) = crate::test_allocation_counter::count_allocations(|| {
        RequestState::arm(Arc::clone(&tight), Arc::clone(&epoch), [2; 32])
    });
    assert!(refused.is_none());
    assert_eq!(allocations, 0);
}

#[test]
fn header_decode_and_owner_wrapper_requested_capacity_fit_precharged_work() {
    let (store, _epoch, mut state) = fixture(1024);
    let mut header = response();
    for number in 0..61 {
        header
            .append_header(
                http::HeaderName::from_bytes(format!("x-{number}").as_bytes()).unwrap(),
                "a",
            )
            .unwrap();
    }
    assert_eq!(header.headers.len(), MAX_FIELDS);
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    let before = state.pending.as_ref().unwrap().entry.arena.as_ptr();
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| state.publish());
    assert!(stats.requested_bytes <= ENTRY_CONTROL);
    let (hit, stats) = crate::test_allocation_counter::count_allocation_stats(|| state.lookup().unwrap());
    assert!(stats.requested_bytes <= HEADER_WORK + READER_CONTROL);
    assert!(hit.header.headers.capacity() >= MAX_FIELDS);
    assert!(std::mem::size_of::<Entry>() + 16 <= ENTRY_CONTROL);
    assert!(std::mem::size_of::<Reader>() + 128 <= READER_CONTROL);
    assert_eq!(
        hit.entry.arena.as_ptr(),
        before,
        "publication moves the arena without a body copy"
    );
    assert_eq!(store.stats().2, 1);
}

#[test]
fn suspended_intents_and_hit_header_refusal_preserve_exact_capacity_ledger() {
    let control = std::mem::size_of::<RequestState>() + 16;
    let store = Store::new(1, 8, 8192 + std::mem::size_of::<Epoch>() + 16 + control * 3, 32768);
    let epoch = store.epoch();
    let intents: Vec<_> = (0..3)
        .map(|_| RequestState::arm(store.clone(), epoch.clone(), [1; 32]).unwrap())
        .collect();
    let (refused, calls) =
        crate::test_allocation_counter::count_allocations(|| RequestState::arm(store.clone(), epoch.clone(), [2; 32]));
    assert!(refused.is_none());
    assert_eq!(calls, 0);
    drop(intents);
    assert!(RequestState::arm(store, epoch, [2; 32]).is_some());

    let store = Store::new(1, 8, 32768, 32768);
    let epoch = store.epoch();
    let mut state = RequestState::arm(store.clone(), epoch, [1; 32]).unwrap();
    let header = response();
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let before = store.stats();
    let (hit, calls) = crate::test_allocation_counter::count_allocations(|| state.lookup());
    assert!(
        hit.is_none(),
        "header-work refusal is an optional miss before downstream commit"
    );
    assert_eq!(calls, 0);
    assert_eq!(store.stats(), before);
}

#[test]
fn default_pending_budget_admits_fifteen_full_arenas_and_refuses_sixteenth() {
    let store = Store::new(64, MAX_BODY, 128 * 1024 * 1024, 16 * 1024 * 1024);
    let epoch = store.epoch();
    let header = response();
    let mut writers = Vec::new();
    for number in 0..16 {
        let mut state = RequestState::arm(store.clone(), epoch.clone(), [number; 32]).unwrap();
        let (_, allocations) =
            crate::test_allocation_counter::count_allocations(|| state.capture(&header, Instant::now()));
        if number == 15 {
            assert!(state.pending.is_none());
            assert_eq!(allocations, 0);
        } else {
            assert_eq!(state.pending.as_ref().unwrap().entry.arena.capacity(), MAX_BODY);
        }
        writers.push(state);
    }
    assert_eq!(store.stats().1, 15 * (MAX_BODY + MAX_METADATA + ENTRY_CONTROL));
    drop(writers);
    assert_eq!(store.stats().1, 0);
}

#[test]
fn replacement_and_expiration_remove_visibility_but_retained_slice_pins_old_arena() {
    let (store, epoch, mut state) = fixture(1024);
    let mut header = response();
    header
        .insert_header(
            "date",
            httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(1)),
        )
        .unwrap();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"old")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let hit = state.lookup().unwrap();
    let retained = hit.body.slice(0..3);
    let before = store.stats().0;
    state.max_ttl = Duration::from_millis(5);
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"new")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    assert_eq!(store.stats().3, 1);
    assert!(
        store.stats().0 > before,
        "replacement keeps the evicted reader arena fully charged"
    );
    assert_eq!(&retained[..], b"old");
    std::thread::sleep(Duration::from_millis(10));
    assert!(state.lookup().is_none());
    assert_eq!(store.stats().3, 0);
    drop(hit);
    drop(state);
    drop(epoch);
    assert_eq!(store.stats().2, 1);
    assert!(store.stats().0 > store.stats().4);
    drop(retained);
    let (total, pending, readers, entries, infrastructure) = store.stats();
    assert_eq!((total, pending, readers, entries), (infrastructure, 0, 0, 0));
}

#[test]
fn metadata_and_reader_control_refusals_allocate_nothing_and_release_work_reservation() {
    let (_, _, mut state) = fixture(1024);
    let mut header = response();
    header
        .insert_header("x-too-large", "x".repeat(MAX_METADATA + 1))
        .unwrap();
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| state.capture(&header, Instant::now()));
    assert_eq!(allocations, 0);
    assert!(state.pending.is_none());

    let base = 8192
        + std::mem::size_of::<Epoch>()
        + 16
        + std::mem::size_of::<RequestState>()
        + 16
        + 8
        + MAX_METADATA
        + ENTRY_CONTROL;
    let store = Store::new(1, 8, base + HEADER_WORK + READER_CONTROL - 1, 32768);
    let epoch = store.epoch();
    let mut state = RequestState::arm(store.clone(), epoch, [3; 32]).unwrap();
    let header = response();
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    assert_eq!(store.stats().0, base);
    let (hit, allocations) = crate::test_allocation_counter::count_allocations(|| state.lookup());
    assert!(hit.is_none());
    assert_eq!(allocations, 0);
    assert_eq!(
        store.stats().0,
        base,
        "header work lease rolls back when reader-control admission refuses"
    );
}

#[test]
fn corrupt_metadata_never_decodes_a_valid_prefix_and_rolls_back_all_temporary_leases() {
    let valid_tail = [0, 3, 0, 0, 0, 2, b'x', b'-', b'a', b'o', b'k'];
    let mut tails = (1..valid_tail.len())
        .map(|length| valid_tail[..length].to_vec())
        .collect::<Vec<_>>();
    tails.extend([
        vec![0, 1, 0, 0, 0, 1, b' ', b'x'],
        vec![0, 1, 0, 0, 0, 1, b'x', b'\n'],
        vec![0, 0, 0, 0, 0, 1, b'x'],
    ]);
    for tail in tails {
        let (store, _epoch, mut state) = fixture(1024);
        let baseline = store.stats().0;
        let header = response();
        state.capture(&header, Instant::now());
        state.push(Some(&Bytes::from_static(b"private")), true);
        assert!(state.seal_pending_headers(&header));
        assert!(state.prepare_publication(Some(&header)));
        state.publish();
        store.corrupt_tail(&tail);
        assert!(
            state.lookup().is_none(),
            "malformed suffix cannot be treated as clean metadata EOF: {tail:?}"
        );
        let (total, pending, readers, entries, _) = store.stats();
        assert_eq!((total, pending, readers, entries), (baseline, 0, 0, 0));
    }
}

#[test]
fn corrupt_body_bound_and_excess_metadata_fields_are_rejected_before_replay() {
    for corrupt_body in [false, true] {
        let (store, _epoch, mut state) = fixture(1024);
        let baseline = store.stats().0;
        let header = response();
        state.capture(&header, Instant::now());
        state.push(None, true);
        assert!(state.seal_pending_headers(&header));
        assert!(state.prepare_publication(Some(&header)));
        state.publish();
        let mut entry = store.entries.remove(&[7; 32]).unwrap();
        let corrupt = Arc::get_mut(&mut entry).unwrap();
        if corrupt_body {
            corrupt.arena.reserve_exact(MAX_BODY + 1);
        } else {
            for _ in 0..MAX_FIELDS {
                corrupt.metadata.extend_from_slice(&[0, 1, 0, 0, 0, 1, b'x', b'a']);
            }
        }
        store.entries.insert([7; 32], entry, Some(Duration::from_secs(60)));
        assert!(state.lookup().is_none());
        assert_eq!(store.stats().0, baseline);
        assert_eq!(store.stats().3, 0);
    }
}

#[test]
fn stale_reader_corruption_veto_cannot_remove_a_newer_replacement() {
    let (store, _epoch, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"old")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let old = store.entries.get(&[7; 32]).unwrap();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"new")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    store.remove_if_current(&[7; 32], &old);
    assert_eq!(store.stats().3, 1);
    assert_eq!(&state.lookup().unwrap().body[..], b"new");
}

impl Store {
    pub(in crate::response_cache) fn publication_epoch(&self) -> PublicationEpoch {
        self.advance_epoch()
    }

    pub(in crate::response_cache) fn lookup_count(&self) -> u64 {
        let stats = self.entries.stats();
        stats.hits + stats.misses + stats.expirations
    }

    pub(in crate::response_cache) fn corrupt_tail(&self, tail: &[u8]) {
        let mut entry = self.entries.remove(&[7; 32]).unwrap();
        let entry_mut = Arc::get_mut(&mut entry).unwrap();
        assert!(entry_mut.metadata.len() + tail.len() <= entry_mut.metadata.capacity());
        entry_mut.metadata.extend_from_slice(tail);
        self.entries.insert([7; 32], entry, Some(Duration::from_secs(60)));
    }

    pub(in crate::response_cache) fn new(
        entries: usize,
        body_cap: usize,
        limit: usize,
        pending_limit: usize,
    ) -> Arc<Self> {
        assert!(entries > 0 && body_cap > 0 && body_cap <= MAX_BODY);
        // Local fixtures retain the historical one-shard index reservation;
        // the standing epoch adds its full control charge to infrastructure.
        let base = entries * 4096 + 4096;
        let infrastructure = base + epoch_control_bytes();
        assert!(limit >= infrastructure);
        let ledger = Arc::new(Ledger {
            total: AtomicUsize::new(base),
            pending: AtomicUsize::new(0),
            readers: AtomicUsize::new(0),
            limit,
            pending_limit,
        });
        let initial = Epoch::admit(&ledger, false).unwrap();
        Arc::new(Self {
            entries: ShardedLruCache::new(entries, 1),
            fence: Mutex::new(initial),
            ledger,
            infrastructure,
            body_cap,
            metadata_cap: MAX_METADATA,
        })
    }
    pub(in crate::response_cache) fn epoch(&self) -> Arc<Epoch> {
        let holder = self.fence.lock();
        assert!(self.entries.is_empty());
        holder.active.store(true, Ordering::Release);
        Arc::clone(&holder)
    }
    pub(in crate::response_cache) fn retire(&self, epoch: &Epoch) {
        let _guard = self.fence.lock();
        epoch.active.store(false, Ordering::Release);
        self.entries.clear();
    }
    pub(in crate::response_cache) fn stats(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.ledger.total.load(Ordering::Acquire),
            self.ledger.pending.load(Ordering::Acquire),
            self.ledger.readers.load(Ordering::Acquire),
            self.entries.len(),
            self.infrastructure,
        )
    }
}

impl RequestState {
    pub(in crate::response_cache) fn arm(store: Arc<Store>, epoch: Arc<Epoch>, key: Key) -> Option<Box<Self>> {
        let lease = Lease::acquire(&store.ledger, std::mem::size_of::<Self>() + 16, false)?;
        Some(Box::new(Self {
            store,
            epoch,
            key,
            max_ttl: Duration::from_secs(60),
            intent: None,
            key_ready: true,
            header_certificate: None,
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
        }))
    }
    pub(in crate::response_cache) fn publish(&mut self) {
        let Some(pending) = self.take_pending() else {
            return;
        };
        self.publish_pending(pending);
    }
}

#[test]
fn standing_epoch_refusal_aliases_then_replacement_charges_distinct_retired_guard() {
    let store = Store::new(1, 8, 128 * 1024, 32768);
    let baseline = store.infrastructure;
    let old = store.advance_epoch();
    assert_ne!(old.id, 0);
    assert!(old.guard.as_ref().unwrap().active.load(Ordering::Acquire));
    assert_eq!(store.stats().0, baseline);
    let pressure = Lease::acquire(&store.ledger, store.ledger.limit - baseline, false).unwrap();
    let (refused, calls) = crate::test_allocation_counter::count_allocations(|| store.advance_epoch());
    assert_eq!(calls, 0, "refusal allocates no epoch owner");
    assert_ne!(refused.id, old.id);
    assert!(Arc::ptr_eq(
        old.guard.as_ref().unwrap(),
        refused.guard.as_ref().unwrap()
    ));
    assert!(!old.guard.as_ref().unwrap().active.load(Ordering::Acquire));
    drop(pressure);
    assert_eq!(store.stats().0, baseline, "aliases are not distinct allocations");
    let replacement = store.advance_epoch();
    assert_ne!(replacement.id, refused.id);
    assert!(!Arc::ptr_eq(
        old.guard.as_ref().unwrap(),
        replacement.guard.as_ref().unwrap()
    ));
    assert_eq!(store.stats().0, baseline + epoch_control_bytes());
    drop(refused);
    assert_eq!(store.stats().0, baseline + epoch_control_bytes());
    drop(old);
    assert_eq!(store.stats().0, baseline);
    let first_clone = replacement.clone();
    let second_clone = replacement.clone();
    assert_eq!(store.stats().0, baseline);
    let later = store.advance_epoch();
    assert_eq!(store.stats().0, baseline + epoch_control_bytes());
    drop(replacement);
    drop(first_clone);
    assert_eq!(store.stats().0, baseline + epoch_control_bytes());
    drop(second_clone);
    assert_eq!(store.stats().0, baseline);
    drop(later);
    assert_eq!(store.stats().0, baseline, "holder retains exactly one standing owner");
    assert!(std::mem::size_of::<Epoch>() + 16 <= epoch_control_bytes());
}

#[test]
fn refused_epoch_publication_clears_entries_and_fences_private_pending_commit() {
    let (store, old, mut state) = fixture(1024);
    let header = response();
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"first")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    assert_eq!(store.stats().3, 1);
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"pending")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    let pressure = Lease::acquire(&store.ledger, store.ledger.limit - store.stats().0, false).unwrap();
    let (token, calls) = crate::test_allocation_counter::count_allocations(|| store.advance_epoch());
    assert_eq!(calls, 0);
    assert!(!token.guard.as_ref().unwrap().active.load(Ordering::Acquire));
    assert!(!old.active.load(Ordering::Acquire));
    assert_eq!(store.stats().3, 0);
    state.publish();
    assert_eq!(store.stats().3, 0, "retired pending writer cannot resurrect visibility");
    assert!(state.lookup().is_none());
    drop(pressure);
    drop(state);
    drop(old);
    drop(token);
    let (total, pending, readers, entries, baseline) = store.stats();
    assert_eq!((total, pending, readers, entries), (baseline, 0, 0, 0));
}

#[test]
fn response_cache_missing_or_drifting_actual_origin_evidence_never_admits_or_publishes_arena() {
    use super::super::key::{DiscoveryStamp, OriginStamp, TlsResult, TlsStamp, TrafficStamp};
    use crate::plugins::http::response_cache::ResponseCachePolicy;
    let discovery = DiscoveryStamp {
        fingerprint: [1; 32],
        membership: 7,
        service: [2; 32],
        section: [3; 32],
    };
    let traffic = TrafficStamp {
        generation: 4,
        result: None,
    };
    let tls = TlsStamp {
        generation: 5,
        discovery: Some(discovery),
        result: TlsResult::Absent,
    };
    let expected = OriginStamp {
        discovery,
        traffic,
        tls,
    };
    let (_, _, mut state) = fixture(1024);
    state.intent = Some(ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig::default()).unwrap());
    state.expected_origin = Some(expected);
    state.expected_authority = super::super::key::authority_stamp(None);
    let header = response();
    state.capture(&header, Instant::now());
    assert!(
        state.pending.is_none(),
        "expected labels alone are not consumed-origin evidence"
    );
    state.record_discovery(Some(discovery));
    state.record_tls(tls);
    state.record_traffic(Some(traffic));
    state.observe_authority(Ok(None));
    state.capture(&header, Instant::now());
    assert!(state.pending.is_some());
    assert!(state.seal_pending_headers(&header));
    state.push(None, true);
    state.record_traffic(Some(TrafficStamp {
        generation: 6,
        result: Some([8; 32]),
    }));
    assert!(
        state.pending.is_none(),
        "later actual absent-to-present lookup abandons private fill"
    );
    state.record_traffic(Some(traffic));
    state.capture(&header, Instant::now());
    assert!(
        state.pending.is_none(),
        "drift bypass is permanent even if a later read matches"
    );
    assert!(!state.seal_pending_headers(&header));
    assert!(!state.prepare_publication(Some(&header)));
    assert_eq!(state.store.stats().3, 0);
}

#[test]
fn response_cache_authority_actual_absence_result_error_and_drift_fence_publication() {
    use super::super::key::{authority_stamp, DiscoveryStamp, OriginStamp, TlsResult, TlsStamp, TrafficStamp};
    use crate::plugins::http::response_cache::ResponseCachePolicy;
    for actual in [
        Ok(None),
        Ok(Some("tenant.sandbox.internal")),
        Ok(Some("other.sandbox.internal")),
        Err(()),
    ] {
        let (_, _, mut state) = fixture(1024);
        let discovery = DiscoveryStamp {
            fingerprint: [1; 32],
            membership: 7,
            service: [2; 32],
            section: [3; 32],
        };
        let traffic = TrafficStamp {
            generation: 4,
            result: None,
        };
        let tls = TlsStamp {
            generation: 5,
            discovery: Some(discovery),
            result: TlsResult::Absent,
        };
        state.intent = Some(ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig::default()).unwrap());
        state.expected_origin = Some(OriginStamp {
            discovery,
            traffic,
            tls,
        });
        state.expected_authority = authority_stamp(Some("tenant.sandbox.internal"));
        state.record_discovery(Some(discovery));
        state.record_traffic(Some(traffic));
        state.record_tls(tls);
        state.capture(&response(), Instant::now());
        assert!(state.pending.is_none(), "unreported actual authority cannot fill");
        state.observe_authority(actual);
        state.capture(&response(), Instant::now());
        if actual == Ok(Some("tenant.sandbox.internal")) {
            assert!(state.pending.is_some());
            assert!(state.seal_pending_headers(&response()));
            state.push(None, true);
            state.observe_authority(Ok(None));
            assert!(state.pending.is_none(), "later absence discards the admitted arena");
            state.observe_authority(Ok(Some("tenant.sandbox.internal")));
            state.capture(&response(), Instant::now());
        }
        assert!(state.pending.is_none());
        assert!(!state.seal_pending_headers(&response()));
        assert!(!state.prepare_publication(Some(&response())));
        assert_eq!(state.store.stats().3, 0);
    }
}

#[test]
fn response_cache_vary_requires_hashed_header_certificate_and_decode_integrity() {
    use super::super::key::HeaderCertificate;
    let (store, _, mut state) = fixture(1024);
    let mut header = response();
    header.insert_header("vary", "X-Public").unwrap();
    state.capture(&header, Instant::now());
    assert!(state.pending.is_none(), "no key certificate cannot admit a Vary writer");
    let mut certificate = HeaderCertificate::default();
    certificate.add("x-public").unwrap();
    state.header_certificate = Some(certificate);
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"public")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    assert_eq!(&state.lookup().unwrap().body[..], b"public");
    store.corrupt_tail(&[
        0, 4, 0, 0, 0, 7, b'v', b'a', b'r', b'y', b'x', b'-', b'o', b't', b'h', b'e', b'r',
    ]);
    assert!(
        state.lookup().is_none(),
        "valid but changed Vary metadata is corrupt before replay"
    );
    assert_eq!(store.stats().3, 0);
    state.capture(&header, Instant::now());
    state.push(Some(&Bytes::from_static(b"selected")), true);
    assert!(state.seal_pending_headers(&header));
    assert!(state.prepare_publication(Some(&header)));
    state.publish();
    let mut hit = state.lookup().unwrap();
    hit.header.insert_header("vary", "x-private").unwrap();
    state.retain_header_charge(&mut hit);
    assert_eq!(
        store.stats().3,
        0,
        "current unsupported response Vary evicts the selected entry"
    );
    assert_eq!(
        &hit.body[..],
        b"selected",
        "selected processed payload remains available exactly once"
    );
}

#[test]
fn response_cache_startup_store_empty_baseline_includes_actual_index_and_standing_epoch_capacity() {
    let limits = crate::cli::config::ResponseCacheLimits::default();
    let infrastructure = limits.validate().unwrap();
    let (store, allocation) =
        crate::test_allocation_counter::count_allocation_stats(|| Store::from_limits(&limits).unwrap());
    assert!(allocation.requested_bytes <= infrastructure);
    assert_eq!(store.stats(), (infrastructure, 0, 0, 0, infrastructure));
    assert!(!store.fence.lock().active.load(Ordering::Acquire));
    assert_eq!(store.body_cap, MAX_BODY);
    assert_eq!(store.metadata_cap, MAX_METADATA);
    let mut invalid = limits;
    invalid.pending_bytes = invalid.total_bytes;
    let (refused, calls) = crate::test_allocation_counter::count_allocations(|| Store::from_limits(&invalid));
    assert!(refused.is_err());
    assert_eq!(calls, 0, "invalid physical limits fail before owner allocation");
}

#[test]
fn pretransport_seal_preserves_strict_headers_and_origin_freshness() {
    let (store, _, mut state) = fixture(1024);
    let mut header = response();
    header
        .insert_header(
            "date",
            httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(5)),
        )
        .unwrap();
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(
        !state.prepare_publication(Some(&header)),
        "unsealed fills are never published"
    );
    assert_eq!(store.stats().1, 0);
    state.capture(&header, Instant::now());
    assert!(state.seal_pending_headers(&header));
    state.push(None, true);
    let mut transported = header.clone();
    transported
        .insert_header("date", httpdate::fmt_http_date(SystemTime::now()))
        .unwrap();
    let entry = &state.pending.as_ref().unwrap().entry;
    assert!(!entry.matches_contract(&transported), "generic integrity remains exact");
    assert!(entry.matches_final_transport_contract(&transported));
    let remaining = entry.freshness.remaining(Instant::now()).unwrap();
    assert!(
        remaining <= Duration::from_secs(55),
        "transport Date cannot refresh origin TTL"
    );
    assert!(state.prepare_publication(Some(&transported)));
    state.publish();
    let hit = state.lookup().unwrap();
    assert!(
        hit.entry.freshness.age(Instant::now()).unwrap() >= Duration::from_secs(5),
        "replay Age is derived from stored origin freshness"
    );
    drop(hit);
    state.capture(&header, Instant::now());
    assert!(state.seal_pending_headers(&header));
    state.capture(&header, Instant::now());
    state.push(None, true);
    assert!(
        !state.prepare_publication(Some(&transported)),
        "recapture invalidates previous seal"
    );
}

#[test]
fn sealed_transport_rejects_every_change_except_valid_singleton_date() {
    for case in 0..8 {
        let (store, _, mut state) = fixture(1024);
        let header = response();
        state.capture(&header, Instant::now());
        assert!(state.seal_pending_headers(&header));
        state.push(None, true);
        let mut final_header = header.clone();
        match case {
            0 => {
                final_header.remove_header("date");
            }
            1 => {
                final_header.insert_header("date", "invalid").unwrap();
            }
            2 => {
                final_header
                    .append_header("date", "Tue, 14 Nov 2023 22:13:20 GMT")
                    .unwrap();
            }
            3 => {
                final_header
                    .insert_header("cache-control", "public, max-age=30")
                    .unwrap();
            }
            4 => {
                final_header.insert_header("age", "1").unwrap();
            }
            5 => {
                final_header.set_status(201).unwrap();
            }
            6 => {
                final_header.insert_header("set-cookie", "session=fixed").unwrap();
            }
            _ => {
                state.pending.as_mut().unwrap().entry.metadata.push(0);
            }
        }
        assert!(!state.prepare_publication(Some(&final_header)), "case {case}");
        assert_eq!(store.stats().1, 0, "case {case} releases pending ownership");
        assert_eq!(store.stats().3, 0);
    }
}

#[test]
fn pretransport_plugin_header_mutation_discards_fill_and_expired_origin_stays_uncacheable() {
    for name in ["date", "cache-control", "age"] {
        let (store, _, mut state) = fixture(1024);
        let header = response();
        state.capture(&header, Instant::now());
        let mut changed = header.clone();
        changed
            .insert_header(
                name,
                match name {
                    "date" => "Tue, 14 Nov 2023 22:13:20 GMT",
                    "cache-control" => "public, max-age=30",
                    _ => "1",
                },
            )
            .unwrap();
        assert!(!state.seal_pending_headers(&changed));
        assert_eq!(store.stats().1, 0);
    }
    let (_, _, mut state) = fixture(1024);
    let mut stale = response();
    stale
        .insert_header(
            "date",
            httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(120)),
        )
        .unwrap();
    state.capture(&stale, Instant::now());
    assert!(
        state.pending.is_none(),
        "fresh transport Date cannot rescue stale origin admission"
    );
}

#[test]
fn contract_scans_preserve_first_fields_and_reject_every_malformed_suffix() {
    fn encode(records: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for (name, value) in records {
            encoded.extend_from_slice(&(name.len() as u16).to_be_bytes());
            encoded.extend_from_slice(&(value.len() as u32).to_be_bytes());
            encoded.extend_from_slice(name);
            encoded.extend_from_slice(value);
        }
        encoded
    }
    fn reference(entry: &Entry, header: &ResponseHeader, names: &[&str]) -> bool {
        fields(&entry.metadata).all(|field| field.is_ok())
            && policy::supported_headers(header)
            && names.iter().all(|name| {
                let mut actual = header.headers.get_all(*name).iter();
                let first = actual.next().map(|value| value.as_bytes());
                actual.next().is_none()
                    && first
                        == fields(&entry.metadata)
                            .filter_map(Result::ok)
                            .find(|(key, _)| key.eq_ignore_ascii_case(name.as_bytes()))
                            .map(|(_, value)| value)
            })
    }
    fn final_reference(entry: &Entry, header: &ResponseHeader) -> bool {
        let mut dates = header.headers.get_all("date").iter();
        let valid_date = dates.next().is_some_and(|date| {
            date.to_str()
                .ok()
                .and_then(|date| httpdate::parse_http_date(date).ok())
                .is_some()
        }) && dates.next().is_none();
        valid_date && reference(entry, header, &["cache-control", "age"])
    }

    let (_, _, mut state) = fixture(1024);
    let original = response();
    state.capture(&original, Instant::now());
    let entry = &mut state.pending.as_mut().unwrap().entry;
    let records: Vec<_> = fields(&entry.metadata)
        .map(|field| {
            let (name, value) = field.unwrap();
            (name.to_vec(), value.to_vec())
        })
        .collect();
    let mut variants = vec![encode(&records), Vec::new()];
    let mut reversed = records.clone();
    reversed.reverse();
    variants.push(encode(&reversed));
    let upper: Vec<_> = records
        .iter()
        .map(|(name, value)| (name.to_ascii_uppercase(), value.clone()))
        .collect();
    variants.push(encode(&upper));
    for name in ["cache-control", "date", "age"] {
        for value in [b"".as_slice(), b"different"] {
            for first in [false, true] {
                let mut changed = records.clone();
                changed.insert(
                    if first { 0 } else { changed.len() },
                    (name.as_bytes().to_vec(), value.to_vec()),
                );
                variants.push(encode(&changed));
            }
        }
        variants.push(encode(
            &records
                .iter()
                .filter(|(key, _)| key != name.as_bytes())
                .cloned()
                .collect::<Vec<_>>(),
        ));
    }
    for prefix in 1..entry.metadata.len() {
        variants.push(entry.metadata[..prefix].to_vec());
    }
    let mut complete_records = records.clone();
    complete_records.push((b"age".to_vec(), b"0".to_vec()));
    let complete = encode(&complete_records);
    variants.push(complete.clone());
    for prefix in [&entry.metadata, &complete] {
        for tail in [vec![0], vec![0, 1, 255, 255, 255, 255, b'x'], vec![0, 0, 0, 0, 0, 0]] {
            let mut changed = prefix.clone();
            changed.extend_from_slice(&tail);
            variants.push(changed);
        }
    }

    let mut headers = vec![original.clone()];
    for name in ["cache-control", "date", "age"] {
        let mut missing = original.clone();
        missing.remove_header(name);
        headers.push(missing);
        for value in ["", "different", "0"] {
            let mut changed = original.clone();
            changed.insert_header(name, value).unwrap();
            headers.push(changed.clone());
            changed.append_header(name, value).unwrap();
            headers.push(changed);
        }
    }
    let mut forbidden = original.clone();
    forbidden.insert_header("set-cookie", "x=y").unwrap();
    headers.push(forbidden);
    let mut other_status = original.clone();
    other_status.set_status(403).unwrap();
    headers.push(other_status);
    let mut transported = original.clone();
    transported
        .insert_header("date", "Sun, 06 Nov 1994 08:49:37 GMT")
        .unwrap();
    headers.push(transported);

    for (metadata_index, metadata) in variants.into_iter().enumerate() {
        entry.metadata = metadata;
        for (header_index, header) in headers.iter().enumerate() {
            let expected = (
                reference(entry, header, &["cache-control", "date", "age"]),
                final_reference(entry, header),
            );
            let (actual, allocations) = crate::test_allocation_counter::count_allocations(|| {
                (
                    entry.matches_contract(header),
                    entry.matches_final_transport_contract(header),
                )
            });
            assert_eq!(actual, expected, "metadata {metadata_index}, header {header_index}");
            assert_eq!(allocations, 0, "contract validation must remain allocation-free");
        }
    }
}

#[test]
fn decoded_origin_integrity_does_not_recompute_or_replace_stored_freshness() {
    let (_store, _epoch, mut state) = fixture(8);
    state.capture(&response(), Instant::now());
    let mut entry = state.pending.take().unwrap().entry;
    let origin_at = entry.freshness.header_at;
    // Decode validates metadata; lookup separately enforces the stored expiry.
    entry.freshness.initial_age = Duration::MAX;
    entry.freshness.lifetime = Duration::ZERO;

    let mut cases = Vec::new();
    for date in ["Sun, 06 Nov 1994 08:49:37 GMT", "Fri, 31 Dec 9999 23:59:59 GMT"] {
        for age in [None, Some("0"), Some("18446744073709551615")] {
            let mut header = response();
            header.insert_header("date", date).unwrap();
            header.remove_header("age");
            if let Some(age) = age {
                header.insert_header("age", age).unwrap();
            }
            cases.push((header, true));
        }
    }
    for (name, value) in [
        ("date", "invalid"),
        ("age", "18446744073709551616"),
        ("age", " 1"),
        ("cache-control", "private,max-age=60"),
        ("vary", "*"),
        ("set-cookie", "token=test"),
    ] {
        let mut header = response();
        header.insert_header(name, value).unwrap();
        cases.push((header, false));
    }
    for name in ["date", "cache-control"] {
        let mut header = response();
        header.remove_header(name);
        cases.push((header, false));
    }
    for name in ["date", "age", "cache-control"] {
        let mut header = response();
        let value = if name == "age" {
            header.insert_header(name, "0").unwrap();
            http::HeaderValue::from_static("0")
        } else {
            header.headers.get(name).unwrap().clone()
        };
        header.append_header(name, value).unwrap();
        cases.push((header, false));
    }
    for name in ["date", "age"] {
        let mut header = response();
        header
            .insert_header(name, http::HeaderValue::from_bytes(&[0xff]).unwrap())
            .unwrap();
        cases.push((header, false));
    }

    for (index, (header, valid)) in cases.into_iter().enumerate() {
        entry.metadata.clear();
        for (name, value) in &header.headers {
            entry
                .metadata
                .extend_from_slice(&u16::try_from(name.as_str().len()).unwrap().to_be_bytes());
            entry
                .metadata
                .extend_from_slice(&u32::try_from(value.len()).unwrap().to_be_bytes());
            entry.metadata.extend_from_slice(name.as_str().as_bytes());
            entry.metadata.extend_from_slice(value.as_bytes());
        }
        assert_eq!(entry.decode().is_some(), valid, "case {index}");
        assert_eq!(entry.freshness.header_at, origin_at);
        assert_eq!(entry.freshness.initial_age, Duration::MAX);
        assert_eq!(entry.freshness.lifetime, Duration::ZERO);
        assert!(entry.freshness.remaining(Instant::now()).is_none());
    }
}

#[test]
fn request_target_key_uses_logical_query_before_transport_barrier() {
    let mut ctx = crate::EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/cache".into();
    ctx.request_query.capture(Some("token=secret&keep=1".into())).unwrap();
    let stale = pingora_http::RequestHeader::build("GET", b"/cache?token=secret&keep=1", None).unwrap();
    ctx.request_query.remove_current_names(&["token"]).unwrap();
    let key = |header: &pingora_http::RequestHeader, context: &crate::EdgionHttpContext| {
        let mut builder = super::super::key::Builder::new();
        frame_request_target(&mut builder, header, context).unwrap();
        builder.finish()
    };
    let mut clean = crate::EdgionHttpContext::new();
    clean.request_info.normalized_current_path = "/cache".into();
    clean.request_query.capture(Some("keep=1".into())).unwrap();
    let clean_header = pingora_http::RequestHeader::build("GET", b"/cache?keep=1", None).unwrap();
    assert_eq!(key(&stale, &ctx), key(&clean_header, &clean));
    clean.request_query.replace_raw(Some("keep=2".into())).unwrap();
    assert_ne!(key(&stale, &ctx), key(&clean_header, &clean));
    assert_eq!(stale.uri.query(), Some("token=secret&keep=1"));
    assert!(ctx.request_query.projection_is_dirty());
}
