use super::*;
use crate::conf_mgr::conf_center::file_system::path_registry::ResourceKey;
use crate::conf_mgr::conf_center::traits::CenterApi;
use std::sync::Arc;

const YAML: &str = "apiVersion: v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";

fn seed_conflict(root: &Path, registry: &Arc<PathRegistry>) -> (PathBuf, PathBuf) {
    // Two files for the SAME (kind, ns/name) at different subdirs (a cross-dir
    // collision — they cannot share one directory, identical canonical filename).
    let winner = root.join("default").join("HTTPRoute_default_r1.yaml");
    let loser = root.join("legacy").join("HTTPRoute_default_r1.yaml");
    std::fs::create_dir_all(winner.parent().unwrap()).unwrap();
    std::fs::create_dir_all(loser.parent().unwrap()).unwrap();
    std::fs::write(&winner, YAML).unwrap();
    std::fs::write(&loser, YAML).unwrap();
    let key = ResourceKey::new("HTTPRoute", "default/r1");
    registry.record(key.clone(), winner.clone()); // winner first
    registry.record(key, loser.clone()); // loser second
    (winner, loser)
}

#[tokio::test]
async fn list_methods_drop_recorded_loser_and_match_get_one() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(PathRegistry::new());
    let storage = FileSystemStorage::new(tmp.path().to_path_buf()).with_registry(registry.clone());
    let (_winner, _loser) = seed_conflict(tmp.path(), &registry);

    // get_list_by_kind returns only the winner.
    let by_kind = storage.get_list_by_kind("HTTPRoute", None).await.unwrap();
    assert_eq!(by_kind.items.len(), 1, "get_list_by_kind must drop the loser");
    assert_eq!(by_kind.items[0].name, "r1");

    // get_list_by_kind_ns returns only the winner.
    let by_ns = storage.get_list_by_kind_ns("HTTPRoute", "default", None).await.unwrap();
    assert_eq!(by_ns.items.len(), 1, "get_list_by_kind_ns must drop the loser");

    // list_all returns only the winner.
    let all = storage.list_all(None).await.unwrap();
    assert_eq!(all.items.len(), 1, "list_all must drop the loser");

    // get_one resolves the same winner the list kept (registry winner_of).
    let one = storage.get_one("HTTPRoute", Some("default"), "r1").await.unwrap();
    assert!(one.contains("name: r1"));
}
