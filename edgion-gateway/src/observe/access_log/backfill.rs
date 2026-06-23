//! Access-log backfill: re-ingest fallback-file logs into ES after an outage.
//!
//! When ES is the primary access-log sink and goes unhealthy, first-healthy
//! routing sends entries to the bounded LocalFile fallback. Each fallback line
//! is exactly one ES doc (the same serialized JSON the ES sender would have
//! shipped). An operator triggers backfill during off-peak; it reads the
//! fallback files line-by-line into the ES bulk channel (awaiting send_bulk =
//! natural backpressure), then renames each fully-processed file to
//! `<name>.backfilled-<ts>` so a re-trigger is idempotent at file granularity.
//!
//! Semantics: at-least-once (no stable doc id). The outage-boundary batch may
//! duplicate in ES. Access logs tolerate this; documented in the user guide.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::observe::metrics::global_metrics;

/// Marker suffix appended to a fallback file once fully backfilled.
const BACKFILLED_MARKER: &str = ".backfilled-";

/// Summary returned by a backfill run.
#[derive(Debug, Default, serde::Serialize, PartialEq)]
pub struct BackfillSummary {
    pub files_processed: usize,
    pub lines_backfilled: u64,
    pub lines_failed: u64,
    pub files_remaining: usize,
    pub stopped_reason: Option<String>,
}

/// Discover not-yet-backfilled fallback files for `base_path`: the active file
/// plus any rotated siblings, excluding files that already carry the
/// `.backfilled-` marker. Returns paths sorted ascending so older rotated files
/// replay before the active file (best-effort time order).
///
/// Matching mirrors `local_file::rotation::append_suffix`, which names archives
/// `{stem}.{suffix}.{ext}` — e.g. `es_fallback.1.log` (size) and
/// `es_fallback.2026-01-02.log` (daily) — NOT `{full_name}.{suffix}`. So a
/// rotated sibling starts with `{stem}.` (a real separator, so `es_fallbackX.log`
/// does not match) and ends with `.{ext}`; the active file matches `{file_name}`
/// exactly. The fallback path is assumed dedicated to this sink.
pub fn discover_fallback_files(base_path: &Path) -> Vec<PathBuf> {
    let Some(dir) = base_path.parent() else {
        return Vec::new();
    };
    let Some(file_name) = base_path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let stem = base_path.file_stem().and_then(|s| s.to_str());
    let ext = base_path.extension().and_then(|e| e.to_str());

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains(BACKFILLED_MARKER) {
                return false; // never re-process an already-backfilled file
            }
            if name == file_name {
                return true; // the active file
            }
            // A rotated sibling: `{stem}.{suffix}.{ext}` (or `{stem}.{suffix}`
            // when the base has no extension).
            match (stem, ext) {
                (Some(stem), Some(ext)) => name.starts_with(&format!("{stem}.")) && name.ends_with(&format!(".{ext}")),
                (Some(stem), None) => name.starts_with(&format!("{stem}.")),
                _ => false,
            }
        })
        .collect();
    out.sort();
    out
}

/// Rename a fully-backfilled file to `<name>.backfilled-<ts>`. `ts` is a
/// caller-supplied stamp (epoch seconds) — the engine does not read the clock
/// itself so it stays unit-testable.
pub fn mark_backfilled(path: &Path, ts: u64) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no valid UTF-8 filename"))?;
    let dest = path.with_file_name(format!("{name}{BACKFILLED_MARKER}{ts}"));
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// Run a backfill pass: re-ingest every not-yet-backfilled fallback file under
/// `base_path` into the ES client identified by `es_key`. The caller guarantees
/// ES is healthy at entry; this rechecks per file and stops early if ES degrades
/// again. `ts` is the rename stamp (epoch seconds), passed in to keep the clock
/// out of here.
pub async fn run_backfill(es_key: &str, base_path: &Path, ts: u64) -> BackfillSummary {
    let mut summary = BackfillSummary::default();
    let files = discover_fallback_files(base_path);
    let total = files.len();

    for (idx, file) in files.iter().enumerate() {
        // Re-resolve the ES client each file; stop if it vanished or degraded.
        let Some(client) = crate::link_sys::get_es_client(es_key) else {
            summary.stopped_reason = Some(format!("ES client '{es_key}' not found"));
            summary.files_remaining = total - idx;
            return summary;
        };
        if !client.healthy() {
            summary.stopped_reason = Some("ES became unhealthy mid-backfill".to_string());
            summary.files_remaining = total - idx;
            return summary;
        }

        let Ok(fh) = std::fs::File::open(file) else {
            summary.stopped_reason = Some(format!("cannot open {}", file.display()));
            summary.files_remaining = total - idx;
            return summary;
        };

        let mut file_failed = false;
        for line in BufReader::new(fh).lines() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            // Awaiting send_bulk backpressures on a full channel — natural rate
            // limit so a just-recovered ES is not overwhelmed.
            match client.send_bulk(line).await {
                Ok(()) => {
                    summary.lines_backfilled += 1;
                    global_metrics().access_log_backfill_ingested(1);
                }
                Err(e) => {
                    // Channel closed / client gone: keep the file (do NOT rename)
                    // for a retry, report, and bail out of the whole run.
                    summary.lines_failed += 1;
                    global_metrics().access_log_backfill_failed(1);
                    tracing::warn!(file = %file.display(), error = %e, "backfill: send_bulk failed; keeping file");
                    file_failed = true;
                    break;
                }
            }
        }

        if file_failed {
            summary.stopped_reason = Some("send_bulk failed".to_string());
            summary.files_remaining = total - idx;
            return summary;
        }

        // File fully fed into the bulk channel — mark it so a re-trigger skips
        // it. NOTE: this marks at queue-entry, not ES-ack; a crash during the
        // bulk loop's background retry could still lose queued lines (the
        // module-level at-least-once contract covers this).
        match mark_backfilled(file, ts) {
            Ok(_) => summary.files_processed += 1,
            Err(e) => {
                tracing::warn!(file = %file.display(), error = %e, "backfill: rename failed");
                summary.stopped_reason = Some("rename failed".to_string());
                summary.files_remaining = total - idx;
                return summary;
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn discover_includes_base_and_rotated_excludes_backfilled() {
        let dir = std::env::temp_dir().join("edgion_bf_discover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("es_fallback.log");
        // Active file + rotation-named archives. Rotation (append_suffix) names
        // them `{stem}.{suffix}.{ext}`, so size -> `es_fallback.1.log`, daily ->
        // `es_fallback.2026-01-02.log`. Also an already-backfilled file and an
        // unrelated log that must NOT be picked up.
        write_file(&dir, "es_fallback.log", "a\n");
        write_file(&dir, "es_fallback.1.log", "b\n");
        write_file(&dir, "es_fallback.2026-01-02.log", "c\n");
        write_file(&dir, "es_fallback.1.log.backfilled-123", "d\n");
        write_file(&dir, "unrelated.log", "e\n");

        let found = discover_fallback_files(&base);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"es_fallback.log".to_string()), "active file included");
        assert!(
            names.contains(&"es_fallback.1.log".to_string()),
            "size-rotated included"
        );
        assert!(
            names.contains(&"es_fallback.2026-01-02.log".to_string()),
            "daily-rotated included"
        );
        assert!(
            !names.iter().any(|n| n.contains("backfilled")),
            "skip already-backfilled"
        );
        assert!(!names.contains(&"unrelated.log".to_string()), "skip unrelated stem");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_backfilled_renames_with_marker() {
        let dir = std::env::temp_dir().join("edgion_bf_mark");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_file(&dir, "es_fallback.log", "a\n");
        let dest = mark_backfilled(&p, 999).unwrap();
        assert!(dest.file_name().unwrap().to_string_lossy().ends_with(".backfilled-999"));
        assert!(!p.exists() && dest.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_backfill_stops_when_es_client_missing_and_keeps_file() {
        let dir = std::env::temp_dir().join("edgion_bf_runmissing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("es_fallback.log");
        write_file(&dir, "es_fallback.log", "{\"m\":1}\n");

        // A key that is not registered in the global ES store -> get_es_client None.
        let summary = run_backfill("default/definitely-not-registered", &base, 7).await;

        assert_eq!(summary.files_processed, 0, "no file processed when ES is missing");
        assert!(summary.stopped_reason.as_deref().unwrap_or("").contains("not found"));
        // The fallback file must be kept (not renamed) so it can be retried.
        assert!(base.exists(), "fallback file preserved on ES-missing");
        assert_eq!(
            summary.files_remaining, 1,
            "the single unprocessed file is still pending"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
