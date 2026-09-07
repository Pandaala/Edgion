//! Content-addressed disk cache for OCI manifests and layer blobs.
//!
//! Every file is named by the SHA-256 of its own content
//! (`<hex>.manifest.json` / `<hex>.blob`), so a read always verifies the
//! bytes against the name; a mismatch (disk corruption, tampering) deletes
//! the entry and falls through to a registry pull. Writes are temp-file +
//! atomic rename, safe across processes. A background task purges files not
//! used for `ttl` (mtime is touched on every hit).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};

/// Disk cache settings, installed once at gateway startup.
#[derive(Debug, Clone)]
pub struct OciDiskCacheConfig {
    pub dir: PathBuf,
    pub ttl: Duration,
    pub purge_interval: Duration,
    /// Total-size budget for the cache directory (`0` = uncapped). Enforced
    /// synchronously on every `store`, so the periodic sweep is only a
    /// backstop — the directory can never exceed this between sweeps.
    pub max_bytes: u64,
}

static CONFIG: RwLock<Option<Arc<OciDiskCacheConfig>>> = RwLock::new(None);

/// Floor for the background purge interval. A zero (or sub-second) interval
/// would make [`ensure_purge_task`] sleep for `Duration::ZERO` and spin,
/// resubmitting blocking directory scans with no pacing for the gateway's
/// lifetime. Operators who configure a smaller value are clamped up to this
/// minimum, mirroring how `configure_oci_pull` clamps a zero timeout/attempts
/// to 1.
const MIN_PURGE_INTERVAL: Duration = Duration::from_secs(1);

/// Clamp a configured purge interval to at least [`MIN_PURGE_INTERVAL`],
/// warning once (at install time) when the operator value is below the floor.
fn clamp_purge_interval(requested: Duration) -> Duration {
    if requested < MIN_PURGE_INTERVAL {
        tracing::warn!(
            requested_secs = requested.as_secs(),
            min_secs = MIN_PURGE_INTERVAL.as_secs(),
            "wasm oci cache purge interval below minimum; clamping"
        );
        MIN_PURGE_INTERVAL
    } else {
        requested
    }
}

/// Serializes the check-then-write sequence in [`store`] (and the size
/// backstop in [`purge_once`]): `make_room`'s directory snapshot and the
/// subsequent write must be atomic with respect to other stores, or
/// concurrent stores could each observe "room available" and collectively
/// overshoot the budget — and two stores of the same digest would race the
/// same temp file. Store events are rare (one per newly pulled manifest or
/// blob), so a process-wide lock costs nothing.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// Install the disk cache config (`None` = disabled). Called once at startup;
/// tests may reconfigure.
pub fn configure_oci_disk_cache(cfg: Option<OciDiskCacheConfig>) {
    let cfg = cfg.map(|mut c| {
        c.purge_interval = clamp_purge_interval(c.purge_interval);
        c
    });
    match &cfg {
        Some(c) => tracing::info!(
            dir = %c.dir.display(),
            ttl_secs = c.ttl.as_secs(),
            purge_interval_secs = c.purge_interval.as_secs(),
            "Wasm OCI disk cache enabled"
        ),
        None => tracing::info!("Wasm OCI disk cache disabled (memory-only)"),
    }
    *CONFIG.write() = cfg.map(Arc::new);
}

fn active_config() -> Option<Arc<OciDiskCacheConfig>> {
    CONFIG.read().clone()
}

/// The two content kinds stored in the cache, distinguished by suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    Manifest,
    Blob,
}

impl CacheKind {
    fn suffix(self) -> &'static str {
        match self {
            CacheKind::Manifest => "manifest.json",
            CacheKind::Blob => "blob",
        }
    }
}

fn entry_path(dir: &Path, kind: CacheKind, digest_hex: &str) -> Option<PathBuf> {
    // Defense in depth: digests always come from an already-verified source,
    // but never join anything that is not exactly 64 hex chars.
    if digest_hex.len() != 64 || !digest_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(dir.join(format!("{}.{}", digest_hex.to_ascii_lowercase(), kind.suffix())))
}

/// Return whether `name` is one of the exact filenames this cache can write.
///
/// Final entries use `<digest>.blob` or `<digest>.manifest.json`. Temporary
/// entries use the same `Path::with_extension` forms as [`try_store`]:
/// `<digest>.tmp.<pid>` and `<digest>.manifest.tmp.<pid>`.
fn is_cache_owned_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let name = name.as_bytes();

    for suffix in [b".blob".as_slice(), b".manifest.json".as_slice()] {
        if name.strip_suffix(suffix).is_some_and(is_lowercase_sha256_hex) {
            return true;
        }
    }

    if name.len() >= 64 {
        let (digest, suffix) = name.split_at(64);
        if is_lowercase_sha256_hex(digest) {
            for separator in [b".tmp.".as_slice(), b".manifest.tmp.".as_slice()] {
                if suffix.strip_prefix(separator).is_some_and(is_canonical_positive_pid) {
                    return true;
                }
            }
        }
    }

    false
}

fn is_lowercase_sha256_hex(value: &[u8]) -> bool {
    value.len() == 64
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn is_canonical_positive_pid(value: &[u8]) -> bool {
    if !matches!(value.first(), Some(b'1'..=b'9')) {
        return false;
    }
    value
        .iter()
        .try_fold(0_u32, |pid, byte| {
            let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
            pid.checked_mul(10)?.checked_add(u32::from(digit))
        })
        .is_some()
}

/// Load a cached entry, verifying content against its name. Touches mtime on
/// a hit (feeds the TTL purge). A verification failure deletes the file.
pub fn load(kind: CacheKind, digest_hex: &str) -> Option<Vec<u8>> {
    let cfg = active_config()?;
    let path = entry_path(&cfg.dir, kind, digest_hex)?;
    let bytes = std::fs::read(&path).ok()?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(digest_hex) {
        tracing::warn!(path = %path.display(), "wasm oci disk cache entry corrupt (digest mismatch); deleting");
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let _ = filetime::set_file_mtime(&path, filetime::FileTime::now());
    Some(bytes)
}

/// Store an entry. Best-effort: a failure is a WARN, never a load error.
/// `digest_hex` MUST be the sha256 of `bytes` — callers verify before storing.
/// Synchronous and internally serialized; callers on async threads accept the
/// brief block (store events are cold-path and permit-bounded).
pub fn store(kind: CacheKind, digest_hex: &str, bytes: &[u8]) {
    let Some(cfg) = active_config() else { return };
    ensure_purge_task();
    let Some(path) = entry_path(&cfg.dir, kind, digest_hex) else {
        return;
    };
    let _guard = STORE_LOCK.lock();
    if !make_room(&cfg, bytes.len() as u64) {
        tracing::warn!(
            bytes = bytes.len(),
            max_bytes = cfg.max_bytes,
            "wasm oci entry exceeds the disk cache budget; not cached"
        );
        return;
    }
    if let Err(e) = try_store(&cfg.dir, &path, bytes) {
        tracing::warn!(path = %path.display(), error = %e, "wasm oci disk cache store failed");
    }
}

/// List cache-owned files (entries + stale temp files) with size and mtime,
/// oldest mtime first. `load` touches mtime on every hit, so mtime order is
/// LRU order.
fn cache_entries_by_age(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = rd
        .flatten()
        .filter(|e| is_cache_owned_name(&e.file_name()))
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            Some((e.path(), md.len(), md.modified().ok()?))
        })
        .collect();
    out.sort_by_key(|(_, _, mtime)| *mtime);
    out
}

/// Evict least-recently-used entries until `incoming` more bytes fit under
/// `max_bytes`. Returns `false` when the incoming entry can never fit (the
/// caller then skips caching it — the load itself is unaffected).
fn make_room(cfg: &OciDiskCacheConfig, incoming: u64) -> bool {
    if cfg.max_bytes == 0 {
        return true;
    }
    if incoming > cfg.max_bytes {
        return false;
    }
    let entries = cache_entries_by_age(&cfg.dir);
    let mut total: u64 = entries.iter().map(|(_, len, _)| *len).sum();
    let mut oldest_first = entries.iter();
    while total + incoming > cfg.max_bytes {
        let Some((path, len, _)) = oldest_first.next() else {
            break;
        };
        if std::fs::remove_file(path).is_ok() {
            tracing::info!(path = %path.display(), "wasm oci disk cache entry evicted (size cap)");
            total -= len;
        }
    }
    total + incoming <= cfg.max_bytes
}

fn try_store(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// One purge sweep: delete cache entries (and stale temp files) whose mtime
/// is older than `ttl`. Returns the number of files removed.
pub fn purge_once(cfg: &OciDiskCacheConfig) -> usize {
    let _guard = STORE_LOCK.lock();
    let Ok(entries) = std::fs::read_dir(&cfg.dir) else {
        return 0;
    };
    let mut removed = 0;
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        if !is_cache_owned_name(&entry.file_name()) {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .map(|age| age > cfg.ttl)
            .unwrap_or(false);
        if expired && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(removed, dir = %cfg.dir.display(), "wasm oci disk cache purged");
    }
    // Backstop size enforcement (store() already guards the budget on every
    // write; this only matters if an operator shrank max_bytes at restart).
    let _ = make_room(cfg, 0);
    removed
}

/// Spawn the background purge loop once (requires a tokio runtime; skipped
/// otherwise — the next `store` from async context starts it).
fn ensure_purge_task() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    tokio::spawn(async {
        loop {
            let Some(cfg) = active_config() else { break };
            // Defensive floor: `configure_oci_disk_cache` already clamps the
            // installed interval, but guard here too so a directly-constructed
            // (test / future caller) `Duration::ZERO` can never busy-loop.
            tokio::time::sleep(cfg.purge_interval.max(MIN_PURGE_INTERVAL)).await;
            let Some(cfg) = active_config() else { break };
            let _ = tokio::task::spawn_blocking(move || purge_once(&cfg)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_hex_of(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn cache_owned_filename_classifier_matches_writer_grammar_exactly() {
        let digest = "0123456789abcdef".repeat(4);
        for name in [
            format!("{digest}.blob"),
            format!("{digest}.manifest.json"),
            format!("{digest}.tmp.1"),
            format!("{digest}.tmp.4294967295"),
            format!("{digest}.manifest.tmp.42"),
        ] {
            assert!(is_cache_owned_name(OsStr::new(&name)), "expected cache-owned: {name}");
        }

        let uppercase_digest = digest.to_ascii_uppercase();
        for name in [
            "notes.blob".to_string(),
            format!("prefix-{digest}.blob"),
            format!("{digest}.blob.backup"),
            format!("{digest}.manifest.json.tmp.7"),
            format!("{digest}.tmp."),
            format!("{digest}.tmp.0"),
            format!("{digest}.tmp.01"),
            format!("{digest}.tmp.+1"),
            format!("{digest}.tmp. 1"),
            format!("{digest}.tmp.4294967296"),
            format!("{digest}.manifest.tmp.1x"),
            format!("{uppercase_digest}.blob"),
            format!("{}.blob", &digest[..63]),
            format!("{}g.blob", &digest[..63]),
            "report.tmp.data".to_string(),
        ] {
            assert!(!is_cache_owned_name(OsStr::new(&name)), "expected foreign: {name}");
        }
    }

    /// One test drives the global-config path end to end (store → hit →
    /// corrupt → repull-miss) to avoid parallel tests racing the global.
    #[test]
    fn store_load_verify_and_disabled() {
        let dir = tempfile::tempdir().unwrap();
        // Disabled: everything is a no-op / miss.
        configure_oci_disk_cache(None);
        let payload = b"wasm-bytes";
        let d = digest_hex_of(payload);
        store(CacheKind::Blob, &d, payload);
        assert!(load(CacheKind::Blob, &d).is_none());

        configure_oci_disk_cache(Some(OciDiskCacheConfig {
            dir: dir.path().to_path_buf(),
            ttl: Duration::from_secs(3600),
            purge_interval: Duration::from_secs(3600),
            max_bytes: 0,
        }));
        store(CacheKind::Blob, &d, payload);
        assert_eq!(load(CacheKind::Blob, &d).unwrap(), payload);
        // Manifest kind uses a distinct file, no cross-kind hit.
        assert!(load(CacheKind::Manifest, &d).is_none());
        // A non-hex "digest" never touches the filesystem.
        assert!(load(CacheKind::Blob, "../../etc/passwd").is_none());

        // Corrupt the file: load must delete it and miss.
        let path = dir.path().join(format!("{d}.blob"));
        std::fs::write(&path, b"tampered").unwrap();
        assert!(load(CacheKind::Blob, &d).is_none());
        assert!(!path.exists(), "corrupt entry must be deleted");

        configure_oci_disk_cache(None);
    }

    #[test]
    fn purge_once_removes_only_expired_cache_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = OciDiskCacheConfig {
            dir: dir.path().to_path_buf(),
            ttl: Duration::from_secs(60),
            purge_interval: Duration::from_secs(60),
            max_bytes: 0,
        };
        let expired = [
            dir.path().join(format!("{}.blob", "a".repeat(64))),
            dir.path().join(format!("{}.manifest.json", "b".repeat(64))),
            dir.path().join(format!("{}.tmp.7", "c".repeat(64))),
            dir.path().join(format!("{}.manifest.tmp.42", "d".repeat(64))),
        ];
        let fresh = dir.path().join(format!("{}.blob", "e".repeat(64)));
        let foreign = [
            dir.path().join("keep.txt"),
            dir.path().join("notes.blob"),
            dir.path().join(format!("prefix-{}.blob", "f".repeat(64))),
            dir.path().join(format!("{}.manifest.json.tmp.7", "1".repeat(64))),
            dir.path().join(format!("{}.tmp.01", "2".repeat(64))),
            dir.path().join("report.tmp.data"),
        ];
        for p in expired.iter().chain(std::iter::once(&fresh)).chain(foreign.iter()) {
            std::fs::write(p, b"x").unwrap();
        }
        let past = filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() - 3600, 0);
        for path in expired.iter().chain(foreign.iter()) {
            filetime::set_file_mtime(path, past).unwrap();
        }
        assert_eq!(purge_once(&cfg), expired.len());
        assert!(expired.iter().all(|path| !path.exists()));
        assert!(fresh.exists());
        assert!(
            foreign.iter().all(|path| path.exists()),
            "foreign files must never be touched"
        );
    }

    #[test]
    fn purge_interval_below_floor_is_clamped() {
        // Zero and sub-second intervals (which would spin `ensure_purge_task`)
        // are lifted to the minimum; a normal interval passes through.
        assert_eq!(clamp_purge_interval(Duration::ZERO), MIN_PURGE_INTERVAL);
        assert_eq!(clamp_purge_interval(Duration::from_millis(10)), MIN_PURGE_INTERVAL);
        assert!(clamp_purge_interval(Duration::ZERO) >= Duration::from_secs(1));
        assert_eq!(
            clamp_purge_interval(Duration::from_secs(3600)),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn size_cap_evicts_lru_and_skips_never_fitting_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = OciDiskCacheConfig {
            dir: dir.path().to_path_buf(),
            ttl: Duration::from_secs(3600),
            purge_interval: Duration::from_secs(3600),
            max_bytes: 8,
        };
        let a = dir.path().join(format!("{}.blob", "a".repeat(64)));
        let b = dir.path().join(format!("{}.blob", "b".repeat(64)));
        let foreign = dir.path().join("notes.blob");
        std::fs::write(&a, b"aaaa").unwrap();
        std::fs::write(&b, b"bbbb").unwrap();
        std::fs::write(&foreign, [b'x'; 64]).unwrap();
        // Make `a` unambiguously the LRU entry (mtime granularity guard).
        let past = filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() - 60, 0);
        filetime::set_file_mtime(&a, past).unwrap();
        filetime::set_file_mtime(
            &foreign,
            filetime::FileTime::from_unix_time(past.unix_seconds() - 60, 0),
        )
        .unwrap();
        // 4 more bytes need room: the LRU entry (`a`) is evicted, `b` stays.
        assert!(make_room(&cfg, 4));
        assert!(!a.exists());
        assert!(b.exists());
        assert!(
            foreign.exists(),
            "foreign suffix collisions must not count toward or be evicted by the cache"
        );
        // An entry that can never fit is refused without evicting anything.
        assert!(!make_room(&cfg, 9));
        assert!(b.exists());
        // max_bytes = 0 disables the cap entirely.
        let uncapped = OciDiskCacheConfig { max_bytes: 0, ..cfg };
        assert!(make_room(&uncapped, u64::MAX));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_filename_ending_in_blob_is_foreign() {
        use std::os::unix::ffi::OsStrExt;

        assert!(!is_cache_owned_name(OsStr::from_bytes(b"\xff.blob")));
    }

    // Apple filesystems reject invalid UTF-8 path components with EILSEQ, so
    // the filesystem survival half is exercised on Unix targets that admit
    // such names. The raw-name classifier assertion above runs on all Unix.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn invalid_utf8_filename_ending_in_blob_survives_cache_maintenance() {
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::tempdir().unwrap();
        let cfg = OciDiskCacheConfig {
            dir: dir.path().to_path_buf(),
            ttl: Duration::ZERO,
            purge_interval: Duration::from_secs(60),
            max_bytes: 1,
        };
        let mut name = vec![0xff];
        name.extend_from_slice(b".blob");
        let foreign = dir.path().join(std::ffi::OsString::from_vec(name));
        std::fs::write(&foreign, b"foreign").unwrap();
        let past = filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() - 60, 0);
        filetime::set_file_mtime(&foreign, past).unwrap();

        assert_eq!(purge_once(&cfg), 0);
        assert!(
            foreign.exists(),
            "invalid UTF-8 filenames must never be treated as cache-owned"
        );
        assert!(make_room(&cfg, 1));
        assert!(
            foreign.exists(),
            "size eviction must also preserve invalid UTF-8 filenames"
        );
    }
}
