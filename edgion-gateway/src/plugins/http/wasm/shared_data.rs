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
    bytes_gauge: Option<edgion_common::observe::metrics_facade::GaugeContribution>,
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
            gauge.increase(bytes as f64);
        }
    }

    fn decrement_bytes_gauge(&self, bytes: usize) {
        if let Some(gauge) = &self.bytes_gauge {
            gauge.decrease(bytes as f64);
        }
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/shared_data_test.rs"]
mod tests;
