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
    let temp_dir = tempfile::tempdir().unwrap();
    let dir = temp_dir.path();
    let base = dir.join("es_fallback.log");
    // Active file + rotation-named archives. Rotation (append_suffix) names
    // them `{stem}.{suffix}.{ext}`, so size -> `es_fallback.1.log`, daily ->
    // `es_fallback.2026-01-02.log`. Also an already-backfilled file and an
    // unrelated log that must NOT be picked up.
    write_file(dir, "es_fallback.log", "a\n");
    write_file(dir, "es_fallback.1.log", "b\n");
    write_file(dir, "es_fallback.2026-01-02.log", "c\n");
    write_file(dir, "es_fallback.1.log.backfilled-123", "d\n");
    write_file(dir, "unrelated.log", "e\n");

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
}

#[test]
fn mark_backfilled_renames_with_marker() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dir = temp_dir.path();
    let p = write_file(dir, "es_fallback.log", "a\n");
    let dest = mark_backfilled(&p, 999).unwrap();
    assert!(dest.file_name().unwrap().to_string_lossy().ends_with(".backfilled-999"));
    assert!(!p.exists() && dest.exists());
}

#[tokio::test]
async fn run_backfill_stops_when_es_client_missing_and_keeps_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dir = temp_dir.path();
    let base = dir.join("es_fallback.log");
    write_file(dir, "es_fallback.log", "{\"m\":1}\n");

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
}
