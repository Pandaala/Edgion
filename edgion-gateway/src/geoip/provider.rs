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

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/41_static_item.rs");

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/47_enum_item.rs");

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/57_macro_invocation.rs");

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/62_struct_item.rs");

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/65_impl_item.rs");

#[cfg(test)]
include!("../../tests/support/geoip/provider_test/73_function_item.rs");

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

        // maxminddb 0.30: `lookup` returns a `LookupResult` that must then be
        // `decode`d into the record type; it is not a direct `Option<T>`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Panic) {
                panic!("geoip test lookup fault");
            }
            #[cfg(test)]
            if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Decode) {
                return Err(maxminddb::MaxMindDbError::decoding("geoip test decode fault"));
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
#[path = "../../tests/unit/geoip/provider_test.rs"]
mod tests;
