//! The process-wide GeoIP database provider.
//!
//! Holds a memory-mapped MMDB behind an `ArcSwapOption`. A lookup is one
//! `ArcSwap` load plus one MMDB query — no lock, no allocation on the miss path.
//!
//! Hot swap relies on `rename(2)` atomicity: after a rename the old inode stays
//! alive for any existing mmap, so in-flight lookups continue against the old
//! database until the last `Arc` drops. Writing `db_path` in place instead
//! (truncate + rewrite) causes SIGBUS in a reader, which is not a Rust panic and
//! terminates the process — the operator documentation forbids it.

use arc_swap::ArcSwapOption;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use super::info::{normalize_addr, project, GeoLookup};

#[cfg(test)]
use std::cell::Cell;

/// One loaded database plus the build timestamp used by the staleness metric.
struct LoadedDb {
    reader: maxminddb::Reader<memmap2::Mmap>,
    build_epoch: u64,
}

pub struct GeoIpProvider {
    db: ArcSwapOption<LoadedDb>,
}

static GLOBAL: LazyLock<GeoIpProvider> = LazyLock::new(GeoIpProvider::new);

/// Serializes tests that touch the process-global `GeoIpProvider::global()`.
///
/// `GeoIpProvider::global()` is a single `LazyLock` shared by every test in
/// this process (Cargo runs a crate's tests as parallel threads in one
/// process). Tests that load the fixture into the global and tests that
/// assert the global is empty would otherwise flake against each other; take
/// this lock around any test body that touches `GeoIpProvider::global()`.
#[cfg(test)]
pub static GEO_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Sticky per-thread lookup fault for deterministic decode/panic coverage.
/// Cleared by [`LookupFaultGuard`] on drop so a panicked test cannot leak into
/// the next one sharing the thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupFault {
    Decode,
    Panic,
}

#[cfg(test)]
thread_local! {
    static LOOKUP_FAULT: Cell<Option<LookupFault>> = const { Cell::new(None) };
}

#[cfg(test)]
pub struct LookupFaultGuard;

#[cfg(test)]
impl Drop for LookupFaultGuard {
    fn drop(&mut self) {
        LOOKUP_FAULT.with(|c| c.set(None));
    }
}

/// Inject a sticky lookup fault until the returned guard is dropped.
#[cfg(test)]
pub fn inject_lookup_fault(fault: LookupFault) -> LookupFaultGuard {
    LOOKUP_FAULT.with(|c| c.set(Some(fault)));
    LookupFaultGuard
}

impl GeoIpProvider {
    pub fn new() -> Self {
        Self {
            db: ArcSwapOption::empty(),
        }
    }

    /// The process-wide singleton. The database, its mmap, and the updater task
    /// are one per process, which is why GeoIP config is node-local rather than
    /// a per-GatewayClass CRD field.
    pub fn global() -> &'static GeoIpProvider {
        &GLOBAL
    }

    /// Clear the loaded database, restoring the provider to its unloaded state.
    ///
    /// Test-only: exists solely so tests that exercise `GeoIpProvider::global()`
    /// can reset the process-global singleton between runs, since Cargo runs a
    /// crate's tests as parallel threads sharing one process (see
    /// `GEO_TEST_LOCK`).
    #[cfg(test)]
    pub fn clear(&self) {
        self.db.store(None);
    }

    /// Memory-map `path` and swap it in. Never panics; never exits.
    pub fn load_from(&self, path: &Path) -> Result<(), String> {
        // SAFETY: `open_mmap` is unsafe in maxminddb 0.30 because a concurrent
        // in-place rewrite of the mapped file causes SIGBUS. The operator
        // documentation forbids writing `db_path` in place; the updater only ever
        // renames a fully-written sibling into it.
        let reader =
            unsafe { maxminddb::Reader::open_mmap(path) }.map_err(|e| format!("open mmdb {}: {e}", path.display()))?;
        let meta = reader.metadata();
        if meta.node_count == 0 {
            return Err(format!("mmdb {} has node_count 0", path.display()));
        }
        let loaded = LoadedDb {
            build_epoch: meta.build_epoch,
            reader,
        };
        self.db.store(Some(Arc::new(loaded)));
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.db.load().is_some()
    }

    pub fn build_epoch(&self) -> Option<u64> {
        self.db.load().as_ref().map(|d| d.build_epoch)
    }

    /// Resolve one address.
    ///
    /// MMDB parsing crosses a deserialization boundary over lower-trust bytes on
    /// the data-plane path, so a malformed record could panic inside the reader.
    /// A caught panic, like a decode error, is `GeoLookup::Failed` so consumers
    /// can apply `failOpen` instead of treating it as ordinary missingness.
    pub fn lookup(&self, ip: IpAddr) -> GeoLookup {
        let guard = self.db.load();
        let Some(db) = guard.as_ref() else {
            return GeoLookup::Unavailable;
        };
        let addr = normalize_addr(ip);

        #[cfg(test)]
        if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Decode) {
            return GeoLookup::Failed;
        }

        // maxminddb 0.30: `lookup` returns a `LookupResult` that must then be
        // `decode`d into the record type; it is not a direct `Option<T>`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Panic) {
                panic!("geoip test lookup fault");
            }
            db.reader
                .lookup(addr)
                .and_then(|r| r.decode::<maxminddb::geoip2::City>())
        }));

        match result {
            Ok(Ok(Some(rec))) => GeoLookup::Found(Arc::new(project(&rec))),
            Ok(Ok(None)) => GeoLookup::NotFound,
            Ok(Err(_)) => GeoLookup::Failed,
            Err(_) => {
                crate::observe::metrics::record_geoip_lookup_panic();
                GeoLookup::Failed
            }
        }
    }
}

impl Default for GeoIpProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_city_db() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb")
    }

    #[test]
    fn empty_provider_reports_unavailable() {
        let p = GeoIpProvider::new();
        assert!(!p.is_loaded());
        assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Unavailable);
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let p = GeoIpProvider::new();
        let err = p.load_from(std::path::Path::new("/nonexistent/x.mmdb")).unwrap_err();
        assert!(!err.is_empty());
        assert!(!p.is_loaded());
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.mmdb");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"this is not an mmdb").unwrap();
        drop(f);

        let p = GeoIpProvider::new();
        assert!(p.load_from(&path).is_err());
        assert!(!p.is_loaded());
    }

    #[test]
    fn loaded_db_resolves_a_known_address() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).expect("fixture must load");
        assert!(p.is_loaded());

        match p.lookup("203.0.113.7".parse().unwrap()) {
            GeoLookup::Found(info) => {
                assert_eq!(info.country_iso.as_deref(), Some("GB"));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn unknown_address_is_not_found_not_unavailable() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        assert_eq!(p.lookup("8.8.8.8".parse().unwrap()), GeoLookup::NotFound);
    }

    /// A dual-stack listener hands us the mapped form; it must resolve
    /// identically to the plain IPv4 address.
    #[test]
    fn ipv4_mapped_address_resolves_like_its_ipv4_form() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let plain = p.lookup("203.0.113.7".parse().unwrap());
        let mapped = p.lookup("::ffff:203.0.113.7".parse().unwrap());
        assert_eq!(plain, mapped);
    }

    /// `127.0.0.6` resolves on the City fixture but carries no city/subdivision
    /// record. This is per-record absence, and must route to `onMissing`, not
    /// to the database-unavailable (`failOpen`) path — those are two
    /// different `GeoLookup` outcomes (`Found` with `None` dimensions, vs.
    /// `Unavailable`) and callers must not conflate them.
    #[test]
    fn dimension_absent_on_record_is_ordinary_missingness() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let GeoLookup::Found(info) = p.lookup("127.0.0.6".parse().unwrap()) else {
            panic!("127.0.0.6 must resolve on the City fixture");
        };
        assert_eq!(info.country_iso.as_deref(), Some("US"));
        assert_eq!(info.city, None);
        assert_eq!(info.subdivision, None);
    }

    /// Reloading swaps the active database without disturbing the API.
    #[test]
    fn reload_replaces_the_active_database() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let first = p.build_epoch();
        p.load_from(&fixture_city_db()).unwrap();
        assert_eq!(p.build_epoch(), first);
        assert!(p.is_loaded());
    }

    #[test]
    fn decode_fault_is_failed_not_not_found() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
    }

    #[test]
    fn panic_fault_is_failed_and_increments_panic_counter() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let _fault = inject_lookup_fault(LookupFault::Panic);
            assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
        });
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_geoip_lookup_panic_total"),
            "panic seam must increment edgion_geoip_lookup_panic_total, got:\n{rendered}"
        );
    }

    #[test]
    fn decode_fault_does_not_increment_panic_counter() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let _fault = inject_lookup_fault(LookupFault::Decode);
            assert_eq!(p.lookup("8.8.8.8".parse().unwrap()), GeoLookup::Failed);
        });
        let rendered = handle.render();
        assert!(
            !rendered.contains("edgion_geoip_lookup_panic_total"),
            "decode seam must not increment the panic counter, got:\n{rendered}"
        );
    }

    #[test]
    fn fault_guard_clears_so_later_lookups_are_real() {
        let p = GeoIpProvider::new();
        p.load_from(&fixture_city_db()).unwrap();
        {
            let _fault = inject_lookup_fault(LookupFault::Decode);
            assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
        }
        match p.lookup("203.0.113.7".parse().unwrap()) {
            GeoLookup::Found(_) => {}
            other => panic!("cleared seam must not stick, got {other:?}"),
        }
    }
}
