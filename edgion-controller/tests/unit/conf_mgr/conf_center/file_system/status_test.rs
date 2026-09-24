use super::*;
use tempfile::TempDir;

#[test]
fn test_status_path() {
    let path = Path::new("/conf/HTTPRoute_default_my-route.yaml");
    let status = FileSystemStatusHandler::status_path(path);
    assert_eq!(status, Path::new("/conf/HTTPRoute_default_my-route.yaml.status"));
}

#[tokio::test]
async fn test_cleanup_orphans() {
    let temp_dir = TempDir::new().unwrap();
    let conf_dir = temp_dir.path().to_path_buf();
    let handler = FileSystemStatusHandler::new(conf_dir.clone());

    // Create a config file with status under the canonical nested layout —
    // `write_error_status` → `build_status_path` falls back to nested when
    // there is no registry, so the sibling spec must live in the same dir
    // to avoid being seen as orphaned.
    let ns_dir = conf_dir.join("default");
    std::fs::create_dir_all(&ns_dir).unwrap();
    let config1 = ns_dir.join("HTTPRoute_default_route1.yaml");
    std::fs::write(&config1, "test: content").unwrap();
    handler
        .write_error_status("HTTPRoute", "default/route1", "OK", "Success")
        .await
        .unwrap();

    // Create an orphan status file (no corresponding config) — placed at
    // root to verify cleanup_orphans walks the whole tree, not just nested.
    let orphan_status = conf_dir.join("HTTPRoute_default_deleted.yaml.status");
    std::fs::write(&orphan_status, "conditions: []").unwrap();

    // Verify both exist
    assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
    assert!(orphan_status.exists());

    // Cleanup orphans
    let cleaned = handler.cleanup_orphans().await.unwrap();
    assert_eq!(cleaned, 1);

    // Verify: orphan deleted, valid status remains
    assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
    assert!(!orphan_status.exists());
}

#[tokio::test]
async fn build_status_path_uses_registry_winner_when_present() {
    use crate::conf_mgr::conf_center::file_system::path_registry::{PathRegistry, ResourceKey};
    use std::sync::Arc;

    let dir = TempDir::new().unwrap();
    let conf = dir.path().to_path_buf();
    let nested = conf.join("gateways");
    std::fs::create_dir_all(&nested).unwrap();
    let spec = nested.join("HTTPRoute_default_r1.yaml");
    std::fs::write(&spec, "x: 1").unwrap();

    let registry = Arc::new(PathRegistry::new());
    registry.record(
        ResourceKey::new("HTTPRoute".to_string(), "default/r1".to_string()),
        spec.clone(),
    );

    let h = FileSystemStatusHandler::new(conf.clone()).with_registry(registry);
    let status_path = h.build_status_path("HTTPRoute", "default/r1");
    assert_eq!(status_path, FileSystemStatusHandler::status_path(&spec));
}

#[tokio::test]
async fn build_status_path_falls_back_to_nested_layout_when_no_winner() {
    // Fallback must match where storage writes new specs (nested namespace
    // subdir), so a first-status race before the watcher observes the spec
    // doesn't leave a flat-root orphan beside a nested spec.
    let dir = TempDir::new().unwrap();
    let conf = dir.path().to_path_buf();
    let h = FileSystemStatusHandler::new(conf.clone());

    let namespaced = h.build_status_path("HTTPRoute", "default/r1");
    assert_eq!(
        namespaced,
        conf.join("default").join("HTTPRoute_default_r1.yaml.status")
    );

    let cluster_scoped = h.build_status_path("GatewayClass", "edgion");
    assert_eq!(
        cluster_scoped,
        conf.join("cluster").join("GatewayClass__edgion.yaml.status")
    );
}
