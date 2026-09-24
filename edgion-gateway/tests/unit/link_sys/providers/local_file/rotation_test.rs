use super::*;

#[test]
fn find_next_size_index_ignores_overlapping_base_name() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("a.a");
    fs::write(&base, "active").unwrap();
    fs::write(dir.path().join("a.2.a"), "archive").unwrap();

    assert_eq!(find_next_size_index(&base), 3);
}
