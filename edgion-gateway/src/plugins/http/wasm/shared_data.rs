//! Per-module shared key/value store (proxy-wasm `{get,set}_shared_data`).
//!
//! One `SharedData` instance hangs off each `ModuleEntry`, so every pooled
//! VM of the same module (across requests and the tick task) shares it, and
//! its lifetime follows the module-cache eviction of the entry.
//!
//! Scope: **single gateway process**. This is NOT cluster-level state —
//! cluster-wide coordination (e.g. global rate limiting) remains the domain
//! of `rate_limit_redis`.
//!
//! CAS semantics per the proxy-wasm spec: every stored value carries a
//! monotonically increasing per-key CAS token (starting at 1); a `set` with
//! a nonzero expected CAS fails with `CasMismatch` unless it matches, and a
//! CAS of 0 writes unconditionally. Setting a null value deletes the key.

use std::sync::atomic::{AtomicUsize, Ordering};

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

#[derive(Default)]
pub struct SharedData {
    map: DashMap<String, (Vec<u8>, u32)>,
    /// Approximate real memory currently stored: key + value bytes plus
    /// [`ENTRY_OVERHEAD_BYTES`] per entry.
    total_bytes: AtomicUsize,
    /// Bounded-cardinality module descriptor for the byte-total gauge.
    metrics_label: String,
}

impl SharedData {
    /// Construct with the owning module's metrics label (for the
    /// `edgion_wasm_shared_data_bytes{module}` gauge).
    pub fn new(metrics_label: String) -> Self {
        SharedData {
            metrics_label,
            ..Default::default()
        }
    }

    /// Read a value and its CAS token.
    pub fn get(&self, key: &str) -> Option<(Vec<u8>, u32)> {
        self.map.get(key).map(|e| e.clone())
    }

    /// Write (`Some`) or delete (`None`) a value. `expected_cas == 0` writes
    /// unconditionally; otherwise the stored CAS must match.
    pub fn set(&self, key: &str, value: Option<Vec<u8>>, expected_cas: u32) -> Result<(), SharedDataError> {
        if let Some(v) = &value {
            if v.len() > MAX_SHARED_VALUE_BYTES {
                return Err(SharedDataError::TooLarge);
            }
        }
        // The DashMap entry guard makes the check-and-set of the value+CAS
        // atomic PER KEY. `total_bytes`, however, is a single process-wide
        // atomic touched by writes to DIFFERENT keys (which hold different
        // shard guards), so it must only ever be moved by EXACT atomic deltas
        // (`fetch_add`/`fetch_sub`) — never a load/compute/store, which would
        // lose concurrent updates and could drift the counter low enough that
        // a later `fetch_sub` underflow-wraps it, permanently bricking the
        // module's shared store on the cap check. The cap check itself reads a
        // snapshot: under concurrent multi-key writes it may momentarily admit
        // one extra value beyond the cap, but the counter never drifts and the
        // bound is self-correcting.
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
                        // Only the value size changes on an in-place update
                        // (the key bytes are unchanged and already counted).
                        let projected = self
                            .total_bytes
                            .load(Ordering::Relaxed)
                            .saturating_sub(old_len)
                            .saturating_add(new_len);
                        if projected > MAX_SHARED_TOTAL_BYTES {
                            return Err(SharedDataError::TooLarge);
                        }
                        let next_cas = cas.wrapping_add(1).max(1);
                        occ.insert((v, next_cas));
                        if new_len >= old_len {
                            self.total_bytes.fetch_add(new_len - old_len, Ordering::Relaxed);
                        } else {
                            self.total_bytes.fetch_sub(old_len - new_len, Ordering::Relaxed);
                        }
                    }
                    None => {
                        self.total_bytes
                            .fetch_sub(ENTRY_OVERHEAD_BYTES + key.len() + old_len, Ordering::Relaxed);
                        occ.remove();
                    }
                }
                self.emit_bytes_gauge();
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
                let add = ENTRY_OVERHEAD_BYTES + key.len() + v.len();
                let projected = self.total_bytes.load(Ordering::Relaxed).saturating_add(add);
                if projected > MAX_SHARED_TOTAL_BYTES {
                    return Err(SharedDataError::TooLarge);
                }
                vac.insert((v, 1));
                self.total_bytes.fetch_add(add, Ordering::Relaxed);
                self.emit_bytes_gauge();
                Ok(())
            }
        }
    }

    /// Publish the current byte total to the per-module gauge. Skipped when
    /// no label is set (test-constructed via `default()`).
    fn emit_bytes_gauge(&self) {
        if !self.metrics_label.is_empty() {
            crate::observe::metrics::registry::set_wasm_shared_data_bytes(
                &self.metrics_label,
                self.total_bytes.load(Ordering::Relaxed),
            );
        }
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use std::sync::Arc;
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
