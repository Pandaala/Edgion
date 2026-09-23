//! Plugin-runtime shared key/value store (proxy-wasm `{get,set}_shared_data`).
//!
//! One `SharedData` instance hangs off each `ModuleEntry`, so every pooled
//! VM of that plugin generation (across requests and the tick task) shares it.
//! Separate plugin entries do not share state. The store is released with the
//! last entry holder, including outstanding request leases after replacement.
//!
//! Scope: **single gateway process**. This is NOT cluster-level state —
//! cluster-wide coordination (e.g. global rate limiting) remains the domain
//! of `rate_limit_redis`.
//!
//! CAS semantics per the proxy-wasm spec: every stored value carries an opaque,
//! nonzero token allocated from a store-wide wrapping sequence; a `set` with a
//! nonzero expected CAS fails with `CasMismatch` unless it matches, and a CAS
//! of 0 writes unconditionally. Setting a null value deletes the key.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

/// Max size of one value. Guest-controlled input — the cap bounds a single
/// write the same way `host_abi` bounds guest allocations.
pub const MAX_SHARED_VALUE_BYTES: usize = 64 * 1024;
/// Max total bytes (keys + values + per-entry overhead) one module may hold.
pub const MAX_SHARED_TOTAL_BYTES: usize = 4 * 1024 * 1024;
/// Fixed per-entry accounting surcharge approximating real memory cost beyond
/// the key+value bytes: the map slot holds a String header (24B), a Vec header
/// (24B), and the CAS u32 + padding, plus hashbrown load-factor amortization
/// and allocator rounding of the two heap blocks. Without it, pathological
/// many-tiny-key loads inflate the cap's real RSS by orders of magnitude.
pub const ENTRY_OVERHEAD_BYTES: usize = 128;

/// Outcome of a failed [`SharedData::set`].
#[derive(Debug, PartialEq, Eq)]
pub enum SharedDataError {
    CasMismatch,
    TooLarge,
}

/// Bytes admitted to the module-wide total but not yet published in the map.
/// Dropping an uncommitted reservation restores the exact previous total.
struct TotalBytesReservation<'a> {
    total_bytes: &'a AtomicUsize,
    bytes: usize,
    committed: bool,
}

impl TotalBytesReservation<'_> {
    /// Disarm rollback after the corresponding map mutation is visible.
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for TotalBytesReservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.total_bytes.fetch_sub(self.bytes, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
pub struct SharedData {
    map: DashMap<String, (Vec<u8>, u32)>,
    /// Approximate real memory currently stored: key + value bytes plus
    /// [`ENTRY_OVERHEAD_BYTES`] per entry.
    total_bytes: AtomicUsize,
    /// Last store-wide CAS token allocated to a successful value write.
    /// Zero is reserved for unconditional guest writes and is never issued.
    last_cas_token: AtomicU32,
    /// Shared exporter handle for all live stores with the same module label.
    bytes_gauge: Option<Arc<metrics::Gauge>>,
}

impl SharedData {
    /// Construct with the owning module's metrics label (for the
    /// `edgion_wasm_shared_data_bytes{module}` gauge).
    pub fn new(metrics_label: String) -> Self {
        SharedData {
            bytes_gauge: (!metrics_label.is_empty())
                .then(|| crate::observe::metrics::registry::wasm_shared_data_bytes_gauge(&metrics_label)),
            map: DashMap::new(),
            total_bytes: AtomicUsize::new(0),
            last_cas_token: AtomicU32::new(0),
        }
    }

    /// Read a value and its CAS token.
    pub fn get(&self, key: &str) -> Option<(Vec<u8>, u32)> {
        self.map.get(key).map(|e| e.clone())
    }

    /// Allocate the next nonzero store-wide CAS token. Relaxed ordering is
    /// sufficient: DashMap guards publish values, while this atomic only
    /// provides a unique modification order for opaque tokens.
    fn allocate_cas_token(&self) -> u32 {
        let mut current = self.last_cas_token.load(Ordering::Relaxed);
        loop {
            let next = current.wrapping_add(1).max(1);
            match self
                .last_cas_token
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }

    /// Atomically reserve growth without ever publishing a total above
    /// `limit`. The limit parameter is private so inline tests can exercise a
    /// single-slot race without weakening the production 4 MiB boundary.
    fn try_reserve_bytes_up_to(
        &self,
        bytes: usize,
        limit: usize,
    ) -> Result<TotalBytesReservation<'_>, SharedDataError> {
        let mut current = self.total_bytes.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(SharedDataError::TooLarge);
            };
            if next > limit {
                return Err(SharedDataError::TooLarge);
            }
            match self
                .total_bytes
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    return Ok(TotalBytesReservation {
                        total_bytes: &self.total_bytes,
                        bytes,
                        committed: false,
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Write (`Some`) or delete (`None`) a value. `expected_cas == 0` writes
    /// unconditionally; otherwise the stored CAS must match.
    pub fn set(&self, key: &str, value: Option<Vec<u8>>, expected_cas: u32) -> Result<(), SharedDataError> {
        self.set_with_total_limit(key, value, expected_cas, MAX_SHARED_TOTAL_BYTES)
    }

    fn set_with_total_limit(
        &self,
        key: &str,
        value: Option<Vec<u8>>,
        expected_cas: u32,
        total_limit: usize,
    ) -> Result<(), SharedDataError> {
        if let Some(v) = &value {
            if v.len() > MAX_SHARED_VALUE_BYTES {
                return Err(SharedDataError::TooLarge);
            }
        }
        // DashMap serializes the value+CAS mutation per key. Growth across
        // different shards is reserved through one checked atomic CAS loop so
        // the documented module-wide cap remains a hard concurrent bound.
        let entry = self.map.entry(key.to_string());
        match entry {
            dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                let (stored, cas) = occ.get();
                if expected_cas != 0 && expected_cas != *cas {
                    return Err(SharedDataError::CasMismatch);
                }
                let old_len = stored.len();
                match value {
                    Some(v) => {
                        let new_len = v.len();
                        let reservation = if new_len > old_len {
                            Some(self.try_reserve_bytes_up_to(new_len - old_len, total_limit)?)
                        } else {
                            None
                        };
                        // Allocate only after validation and any required
                        // reservation, immediately before publication.
                        let next_cas = self.allocate_cas_token();
                        let old = occ.insert((v, next_cas));
                        if let Some(reservation) = reservation {
                            // `occ` still owns the shard guard, so no delete can
                            // release these bytes before rollback is disarmed.
                            reservation.commit();
                            self.increment_bytes_gauge(new_len - old_len);
                        }
                        if new_len < old_len {
                            self.total_bytes.fetch_sub(old_len - new_len, Ordering::Relaxed);
                            self.decrement_bytes_gauge(old_len - new_len);
                        }
                        drop(occ);
                        drop(old);
                    }
                    None => {
                        occ.remove();
                        // Release only after deletion is visible.
                        self.total_bytes
                            .fetch_sub(ENTRY_OVERHEAD_BYTES + key.len() + old_len, Ordering::Relaxed);
                        self.decrement_bytes_gauge(ENTRY_OVERHEAD_BYTES + key.len() + old_len);
                    }
                }
                Ok(())
            }
            dashmap::mapref::entry::Entry::Vacant(vac) => {
                // Deleting a missing key with CAS 0 is a no-op; a nonzero
                // CAS on a missing key can never match.
                if expected_cas != 0 {
                    return Err(SharedDataError::CasMismatch);
                }
                let Some(v) = value else {
                    return Ok(());
                };
                let add = ENTRY_OVERHEAD_BYTES
                    .checked_add(key.len())
                    .and_then(|bytes| bytes.checked_add(v.len()))
                    .ok_or(SharedDataError::TooLarge)?;
                let reservation = self.try_reserve_bytes_up_to(add, total_limit)?;
                let next_cas = self.allocate_cas_token();
                let inserted = vac.insert((v, next_cas));
                // Keep the returned shard guard until the reservation can no
                // longer roll back, preventing a concurrent delete from
                // releasing the same bytes first.
                reservation.commit();
                // Publish growth before releasing the key guard so a delete
                // cannot subtract this contribution before it is registered.
                self.increment_bytes_gauge(add);
                drop(inserted);
                Ok(())
            }
        }
    }

    fn increment_bytes_gauge(&self, bytes: usize) {
        if let Some(gauge) = &self.bytes_gauge {
            gauge.increment(bytes as f64);
        }
    }

    fn decrement_bytes_gauge(&self, bytes: usize) {
        if let Some(gauge) = &self.bytes_gauge {
            gauge.decrement(bytes as f64);
        }
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes.load(Ordering::Relaxed)
    }
}

impl Drop for SharedData {
    fn drop(&mut self) {
        // Exclusive destruction means every mutation and reservation has
        // finished. Withdraw only this store's remaining contribution.
        let remaining = *self.total_bytes.get_mut();
        self.decrement_bytes_gauge(remaining);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Barrier};

    use super::*;

    fn assert_gauge(handle: &metrics_exporter_prometheus::PrometheusHandle, module: &str, bytes: usize) {
        let rendered = crate::observe::metrics::registry::with_live_wasm_metrics(|| handle.render());
        let sample = format!("edgion_wasm_shared_data_bytes{{module=\"{module}\"}} {bytes}");
        assert!(
            rendered.lines().any(|line| line == sample),
            "expected {sample}:\n{rendered}"
        );
    }

    #[test]
    fn gauge_aggregates_generations_and_withdraws_only_the_dropped_store() {
        for old_first in [true, false] {
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = recorder.handle();
            metrics::with_local_recorder(&recorder, || {
                let module = format!("generation-{old_first}");
                let old = SharedData::new(module.clone());
                old.set("k", Some(vec![0; 100]), 0).unwrap();
                let new = SharedData::new(module.clone());
                assert_gauge(&handle, &module, 229);
                new.set("k", Some(vec![0; 40]), 0).unwrap();
                assert_gauge(&handle, &module, 398);
                old.set("k", Some(vec![0; 140]), 0).unwrap();
                new.set("k", Some(vec![0; 10]), 0).unwrap();
                assert_gauge(&handle, &module, 408);
                assert_eq!(new.set("missing", Some(vec![0]), 1), Err(SharedDataError::CasMismatch));
                assert_eq!(
                    new.set("large", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), 0),
                    Err(SharedDataError::TooLarge)
                );
                assert_eq!(
                    new.set_with_total_limit("extra", Some(vec![0]), 0, 139),
                    Err(SharedDataError::TooLarge)
                );
                new.set("missing", None, 0).unwrap();
                assert_gauge(&handle, &module, 408);
                if old_first {
                    drop(old);
                    assert_gauge(&handle, &module, 139);
                    new.set("k", None, 0).unwrap();
                    assert_gauge(&handle, &module, 0);
                    new.set("k", Some(vec![0; 10]), 0).unwrap();
                    drop(new);
                } else {
                    drop(new);
                    assert_gauge(&handle, &module, 269);
                    drop(old);
                }
                assert_gauge(&handle, &module, 0);
                let recreated = SharedData::new(module.clone());
                recreated.set("k", Some(vec![0]), 0).unwrap();
                assert_gauge(&handle, &module, 130);
                drop(recreated);
                assert_gauge(&handle, &module, 0);
                assert!(!crate::observe::metrics::registry::wasm_shared_data_metric_stats()
                    .keys
                    .contains(&module));
            });
        }
    }

    #[test]
    fn gauge_concurrent_mutations_registration_and_drop_balance() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let module = "concurrent-shared-data";
        let retained = metrics::with_local_recorder(&recorder, || Arc::new(SharedData::new(module.into())));
        retained.set("retained", Some(vec![0; 20]), 0).unwrap();
        let start = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let recorder = &recorder;
                let start = &start;
                let retained = &retained;
                scope.spawn(move || {
                    start.wait();
                    metrics::with_local_recorder(recorder, || {
                        for _ in 0..100 {
                            let temporary = SharedData::new(module.into());
                            temporary.set("tmp", Some(vec![0; 50]), 0).unwrap();
                            retained.set("contended", Some(vec![0; 30]), 0).unwrap();
                            retained.set("contended", Some(vec![0; 5]), 0).unwrap();
                            retained.set("contended", None, 0).unwrap();
                        }
                    });
                });
            }
        });
        assert_gauge(&handle, module, accounted_map_bytes(&retained));
        // A request-held Arc keeps the contribution after its cache owner drops.
        let request = retained.clone();
        drop(retained);
        assert_gauge(&handle, module, 156);
        drop(request);
        assert_gauge(&handle, module, 0);
    }

    #[test]
    fn gauge_live_store_survives_idle_eviction_and_releases_its_handle() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new()
            .idle_timeout(metrics_util::MetricKindMask::ALL, Some(std::time::Duration::ZERO))
            .build_recorder();
        let handle = recorder.handle();
        let module = "idle-shared-data";
        let store = metrics::with_local_recorder(&recorder, || SharedData::new(module.into()));
        store.set("k", Some(vec![0; 10]), 0).unwrap();
        for _ in 0..4 {
            assert_gauge(&handle, module, 139);
        }
        let overlap = metrics::with_local_recorder(&recorder, || SharedData::new(module.into()));
        overlap.set("k", Some(vec![0; 20]), 0).unwrap();
        drop(store);
        assert_gauge(&handle, module, 149);
        drop(overlap);
        assert_gauge(&handle, module, 0);
        // No owners remain, so a subsequent scrape may evict the zero series.
        let _ = handle.render();
        assert!(!handle.render().contains("edgion_wasm_shared_data_bytes{"));
    }

    #[test]
    fn gauge_last_drop_racing_registration_preserves_the_new_store() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let module = "drop-register-shared-data";
        for _ in 0..32 {
            let old = metrics::with_local_recorder(&recorder, || SharedData::new(module.into()));
            old.set("k", Some(vec![0; 100]), 0).unwrap();
            let start = Barrier::new(2);
            let new = std::thread::scope(|scope| {
                let start_ref = &start;
                scope.spawn(move || {
                    start_ref.wait();
                    drop(old);
                });
                start.wait();
                let new = metrics::with_local_recorder(&recorder, || SharedData::new(module.into()));
                new.set("k", Some(vec![0; 40]), 0).unwrap();
                new
            });
            assert_gauge(&handle, module, 169);
            drop(new);
            assert_gauge(&handle, module, 0);
        }
    }

    fn accounted_map_bytes(sd: &SharedData) -> usize {
        sd.map
            .iter()
            .map(|entry| {
                let (value, _) = entry.value();
                ENTRY_OVERHEAD_BYTES + entry.key().len() + value.len()
            })
            .sum()
    }

    #[test]
    fn set_get_roundtrip_and_cas_increments() {
        let sd = SharedData::default();
        assert!(sd.get("k").is_none());
        sd.set("k", Some(b"v1".to_vec()), 0).unwrap();
        let (v, cas1) = sd.get("k").unwrap();
        assert_eq!(v, b"v1");
        assert_eq!(cas1, 1);
        sd.set("k", Some(b"v2".to_vec()), cas1).unwrap();
        let (v, cas2) = sd.get("k").unwrap();
        assert_eq!(v, b"v2");
        assert!(cas2 > cas1);
    }

    #[test]
    fn cas_mismatch_rejected() {
        let sd = SharedData::default();
        sd.set("k", Some(b"v".to_vec()), 0).unwrap();
        assert_eq!(sd.set("k", Some(b"x".to_vec()), 999), Err(SharedDataError::CasMismatch));
        // Nonzero CAS on a missing key can never match.
        assert_eq!(
            sd.set("missing", Some(b"x".to_vec()), 1),
            Err(SharedDataError::CasMismatch)
        );
    }

    #[test]
    fn delete_recreate_rejects_stale_cas() {
        let sd = SharedData::default();
        sd.set("k", Some(b"old".to_vec()), 0).unwrap();
        let (_, stale_cas) = sd.get("k").unwrap();

        sd.set("k", None, stale_cas).unwrap();
        sd.set("k", Some(b"new".to_vec()), 0).unwrap();
        let (new_value, new_cas) = sd.get("k").unwrap();
        assert_eq!(new_value, b"new");
        assert_ne!(new_cas, stale_cas);

        let total_before = sd.total_bytes();
        assert_eq!(
            sd.set("k", Some(b"stale overwrite".to_vec()), stale_cas),
            Err(SharedDataError::CasMismatch)
        );
        assert_eq!(sd.get("k"), Some((b"new".to_vec(), new_cas)));
        assert_eq!(sd.total_bytes(), total_before);
    }

    #[test]
    fn tokens_wrap_from_max_to_one_then_two() {
        let sd = SharedData::default();
        sd.last_cas_token.store(u32::MAX - 1, Ordering::Relaxed);

        sd.set("max", Some(Vec::new()), 0).unwrap();
        sd.set("one", Some(Vec::new()), 0).unwrap();
        sd.set("two", Some(Vec::new()), 0).unwrap();

        assert_eq!(sd.get("max").unwrap().1, u32::MAX);
        assert_eq!(sd.get("one").unwrap().1, 1);
        assert_eq!(sd.get("two").unwrap().1, 2);
    }

    #[test]
    fn concurrent_tokens_skip_zero_and_do_not_duplicate_across_wrap() {
        const WRITERS: usize = 8;
        let sd = Arc::new(SharedData::default());
        sd.last_cas_token.store(u32::MAX - 4, Ordering::Relaxed);
        let start = Arc::new(Barrier::new(WRITERS + 1));
        let mut handles = Vec::new();

        for index in 0..WRITERS {
            let sd = sd.clone();
            let start = start.clone();
            handles.push(std::thread::spawn(move || {
                let key = format!("wrap-{index}");
                start.wait();
                sd.set(&key, Some(vec![index as u8]), 0).unwrap();
                sd.get(&key).unwrap().1
            }));
        }
        start.wait();

        let tokens: BTreeSet<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        let expected = BTreeSet::from([u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX, 1, 2, 3, 4]);
        assert_eq!(tokens, expected);
        assert!(!tokens.contains(&0));
    }

    #[test]
    fn failures_deletes_and_noops_do_not_consume_tokens() {
        let sd = SharedData::default();
        sd.set("k", Some(b"value".to_vec()), 0).unwrap();
        let (_, cas) = sd.get("k").unwrap();
        assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), cas);

        assert_eq!(
            sd.set("k", Some(b"stale".to_vec()), cas.wrapping_add(1)),
            Err(SharedDataError::CasMismatch)
        );
        assert_eq!(
            sd.set("k", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), cas),
            Err(SharedDataError::TooLarge)
        );
        assert_eq!(
            sd.set_with_total_limit("other", Some(Vec::new()), 0, sd.total_bytes()),
            Err(SharedDataError::TooLarge)
        );
        sd.set("k", None, cas).unwrap();
        sd.set("missing", None, 0).unwrap();
        assert_eq!(sd.set("missing", None, cas), Err(SharedDataError::CasMismatch));
        assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), cas);

        sd.set("next", Some(Vec::new()), 0).unwrap();
        assert_eq!(sd.get("next").unwrap().1, cas.wrapping_add(1).max(1));
    }

    #[test]
    fn reservation_single_slot_is_atomic_and_rolls_back() {
        const SLOT_BYTES: usize = 17;
        let sd = Arc::new(SharedData::default());
        let phase = Arc::new(Barrier::new(2));
        let worker = {
            let sd = sd.clone();
            let phase = phase.clone();
            std::thread::spawn(move || {
                let reservation = sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES).unwrap();
                phase.wait();
                phase.wait();
                drop(reservation);
            })
        };

        phase.wait();
        assert_eq!(sd.total_bytes(), SLOT_BYTES);
        assert!(matches!(
            sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES),
            Err(SharedDataError::TooLarge)
        ));
        phase.wait();
        worker.join().unwrap();
        assert_eq!(sd.total_bytes(), 0);

        let reservation = sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES).unwrap();
        assert_eq!(sd.total_bytes(), SLOT_BYTES);
        drop(reservation);
        assert_eq!(sd.total_bytes(), 0);

        let one_byte = sd.try_reserve_bytes_up_to(1, usize::MAX).unwrap();
        assert!(matches!(
            sd.try_reserve_bytes_up_to(usize::MAX, usize::MAX),
            Err(SharedDataError::TooLarge)
        ));
        drop(one_byte);
        assert_eq!(sd.total_bytes(), 0);
    }

    #[test]
    fn delete_frees_bytes() {
        let sd = SharedData::default();
        sd.set("key", Some(vec![0u8; 100]), 0).unwrap();
        let before = sd.total_bytes();
        assert!(before >= 100);
        sd.set("key", None, 0).unwrap();
        assert!(sd.get("key").is_none());
        assert!(sd.total_bytes() < before);
        // Deleting a missing key is a no-op.
        sd.set("key", None, 0).unwrap();
    }

    #[test]
    fn accounting_tracks_value_shrink_and_delete() {
        let sd = SharedData::default();
        sd.set("k", Some(vec![0u8; 1000]), 0).unwrap();
        let after_big = sd.total_bytes();
        assert_eq!(after_big, ENTRY_OVERHEAD_BYTES + 1000 + "k".len());
        // Shrink the value: the counter MUST drop, not stay at the old size
        // (the old `if new_total >= old_len` guard dropped this decrement).
        sd.set("k", Some(vec![0u8; 10]), sd.get("k").unwrap().1).unwrap();
        assert_eq!(sd.total_bytes(), ENTRY_OVERHEAD_BYTES + 10 + "k".len());
        // Growing the same entry reserves only the positive value delta.
        let shrink_cas = sd.get("k").unwrap().1;
        sd.set("k", Some(vec![0u8; 100]), shrink_cas).unwrap();
        assert_eq!(sd.get("k").unwrap().1, shrink_cas + 1);
        assert_eq!(sd.total_bytes(), ENTRY_OVERHEAD_BYTES + 100 + "k".len());
        // Delete brings it to zero exactly — no phantom residue, no wrap.
        sd.set("k", None, 0).unwrap();
        assert_eq!(sd.total_bytes(), 0);
    }

    #[test]
    fn entry_overhead_bounds_tiny_key_count() {
        // Pathological load: many tiny keys with empty values. Without the
        // per-entry surcharge the logical accounting admits ~4M such entries
        // (hundreds of MB of real memory); with it the entry count is bounded
        // to cap/(overhead+key) and the cap stays honest about RSS.
        let sd = SharedData::default();
        let per_entry = ENTRY_OVERHEAD_BYTES + 8;
        let expected_max = MAX_SHARED_TOTAL_BYTES / per_entry;
        let mut inserted = 0usize;
        loop {
            match sd.set(&format!("{inserted:08}"), Some(Vec::new()), 0) {
                Ok(()) => inserted += 1,
                Err(SharedDataError::TooLarge) => break,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
            assert!(
                inserted <= expected_max,
                "cap admitted more entries than the surcharge allows"
            );
        }
        assert_eq!(inserted, expected_max);
        assert_eq!(sd.total_bytes(), expected_max * per_entry);
    }

    #[test]
    fn concurrent_multikey_writes_keep_exact_total() {
        let sd = Arc::new(SharedData::default());
        let mut handles = Vec::new();
        for t in 0..8 {
            let sd = sd.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..200 {
                    let key = format!("k{t}-{i}");
                    sd.set(&key, Some(vec![0u8; 32]), 0).unwrap();
                    // Shrink then delete: exercises every accounting branch.
                    sd.set(&key, Some(vec![0u8; 8]), sd.get(&key).unwrap().1).unwrap();
                    sd.set(&key, None, 0).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Every key was deleted; with exact atomic deltas the counter must
        // land back at exactly zero regardless of interleaving (a load/store
        // scheme would drift and could underflow-wrap to usize::MAX).
        assert_eq!(
            sd.total_bytes(),
            0,
            "atomic-delta accounting must be exact under concurrency"
        );
        assert_eq!(sd.total_bytes(), accounted_map_bytes(&sd));
    }

    #[test]
    fn concurrent_public_writes_enforce_cap_and_exact_accounting() {
        const WRITERS: usize = 72;
        const KEY_LEN: usize = 8;
        let sd = Arc::new(SharedData::default());
        let start = Arc::new(Barrier::new(WRITERS + 1));
        let mut handles = Vec::new();

        for index in 0..WRITERS {
            let sd = sd.clone();
            let start = start.clone();
            handles.push(std::thread::spawn(move || {
                let key = format!("k{index:07}");
                start.wait();
                (key.clone(), sd.set(&key, Some(vec![0; MAX_SHARED_VALUE_BYTES]), 0))
            }));
        }
        start.wait();

        let mut successes = 0;
        for handle in handles {
            let (key, result) = handle.join().unwrap();
            match result {
                Ok(()) => {
                    successes += 1;
                    assert!(sd.get(&key).is_some());
                }
                Err(SharedDataError::TooLarge) => assert!(sd.get(&key).is_none()),
                Err(error) => panic!("unexpected concurrent write error: {error:?}"),
            }
        }

        let charge = ENTRY_OVERHEAD_BYTES + KEY_LEN + MAX_SHARED_VALUE_BYTES;
        assert_eq!(successes, MAX_SHARED_TOTAL_BYTES / charge);
        assert!(sd.total_bytes() <= MAX_SHARED_TOTAL_BYTES);
        assert_eq!(sd.total_bytes(), successes * charge);
        assert_eq!(sd.total_bytes(), accounted_map_bytes(&sd));
    }

    #[test]
    fn rejected_mutations_leave_value_token_and_accounting_unchanged() {
        let sd = SharedData::default();
        sd.set("k", Some(b"stable".to_vec()), 0).unwrap();
        let snapshot = sd.get("k").unwrap();
        let total = sd.total_bytes();
        let last_token = sd.last_cas_token.load(Ordering::Relaxed);

        let assert_unchanged = |sd: &SharedData| {
            assert_eq!(sd.get("k"), Some(snapshot.clone()));
            assert_eq!(sd.total_bytes(), total);
            assert_eq!(sd.total_bytes(), accounted_map_bytes(sd));
            assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), last_token);
        };

        assert_eq!(
            sd.set("k", Some(b"stale".to_vec()), snapshot.1.wrapping_add(1)),
            Err(SharedDataError::CasMismatch)
        );
        assert_unchanged(&sd);

        assert_eq!(
            sd.set("k", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), snapshot.1),
            Err(SharedDataError::TooLarge)
        );
        assert_unchanged(&sd);

        assert_eq!(
            sd.set_with_total_limit("k", Some(b"stable+".to_vec()), snapshot.1, total),
            Err(SharedDataError::TooLarge)
        );
        assert_unchanged(&sd);

        assert_eq!(
            sd.set_with_total_limit("k", Some(b"stable+".to_vec()), snapshot.1.wrapping_add(1), total),
            Err(SharedDataError::CasMismatch),
            "CAS mismatch remains authoritative over aggregate capacity"
        );
        assert_unchanged(&sd);
    }

    #[test]
    fn size_limits_enforced() {
        let sd = SharedData::default();
        assert_eq!(
            sd.set("big", Some(vec![0u8; MAX_SHARED_VALUE_BYTES + 1]), 0),
            Err(SharedDataError::TooLarge)
        );
        // Fill close to the total cap with max-size values (leaving room
        // for key bytes), then overflow.
        let n = MAX_SHARED_TOTAL_BYTES / MAX_SHARED_VALUE_BYTES - 1;
        for i in 0..n {
            sd.set(&format!("k{i}"), Some(vec![0u8; MAX_SHARED_VALUE_BYTES]), 0)
                .unwrap();
        }
        assert_eq!(
            sd.set("overflow", Some(vec![0u8; MAX_SHARED_VALUE_BYTES]), 0),
            Err(SharedDataError::TooLarge)
        );
    }
}
