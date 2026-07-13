//! Content-addressed disk cache for OCI manifests and layer blobs.
//!
//! Every file is named by the SHA-256 of its own content
//! (`<hex>.manifest.json` / `<hex>.blob`), so a read always verifies the
//! bytes against the name; a mismatch (disk corruption, tampering) deletes
//! the entry and falls through to a registry pull. Writes are temp-file +
//! atomic rename, safe across processes. A background task purges files not
//! used for `ttl` (mtime is touched on every hit).

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
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".manifest.json") || name.ends_with(".blob") || name.contains(".tmp.")
        })
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
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_ours = name.ends_with(".manifest.json") || name.ends_with(".blob") || name.contains(".tmp.");
        if !is_ours {
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
            tokio::time::sleep(cfg.purge_interval).await;
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
        let old = dir.path().join(format!("{}.blob", "a".repeat(64)));
        let fresh = dir.path().join(format!("{}.blob", "b".repeat(64)));
        let foreign = dir.path().join("keep.txt");
        for p in [&old, &fresh, &foreign] {
            std::fs::write(p, b"x").unwrap();
        }
        let past = filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() - 3600, 0);
        filetime::set_file_mtime(&old, past).unwrap();
        assert_eq!(purge_once(&cfg), 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(foreign.exists(), "non-cache files must never be touched");
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
        std::fs::write(&a, b"aaaa").unwrap();
        std::fs::write(&b, b"bbbb").unwrap();
        // Make `a` unambiguously the LRU entry (mtime granularity guard).
        let past = filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() - 60, 0);
        filetime::set_file_mtime(&a, past).unwrap();
        // 4 more bytes need room: the LRU entry (`a`) is evicted, `b` stays.
        assert!(make_room(&cfg, 4));
        assert!(!a.exists());
        assert!(b.exists());
        // An entry that can never fit is refused without evicting anything.
        assert!(!make_room(&cfg, 9));
        assert!(b.exists());
        // max_bytes = 0 disables the cap entirely.
        let uncapped = OciDiskCacheConfig { max_bytes: 0, ..cfg };
        assert!(make_room(&uncapped, u64::MAX));
    }
}
