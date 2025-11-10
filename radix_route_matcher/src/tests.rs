use super::RadixTree;

#[test]
fn insert_and_find_exact() {
    let mut tree = RadixTree::new().expect("create tree");
    tree.insert("/api", 1).expect("insert /api");
    tree.insert("/api/users", 2).expect("insert /api/users");

    assert_eq!(tree.find_exact("/api"), Some(1));
    assert_eq!(tree.find_exact("/api/users"), Some(2));
    assert_eq!(tree.find_exact("/api/posts"), None);
}

#[test]
fn longest_prefix_and_all_prefixes() {
    let mut tree = RadixTree::new().expect("create tree");
    tree.insert("/", 1).unwrap();
    tree.insert("/api", 2).unwrap();
    tree.insert("/api/users", 3).unwrap();

    let iter = tree.create_iter().expect("create iter");
    assert_eq!(tree.longest_prefix(&iter, "/api/users/123"), Some(3));

    let prefixes = tree.find_all_prefixes(&iter, "/api/users/123");
    assert_eq!(prefixes, vec![3, 2, 1]);
}

#[test]
fn remove_routes() {
    let mut tree = RadixTree::new().expect("create tree");
    tree.insert("/foo", 10).unwrap();
    tree.insert("/foo/bar", 11).unwrap();

    tree.remove("/foo").expect("remove /foo");
    assert_eq!(tree.find_exact("/foo"), None);
    assert_eq!(tree.find_exact("/foo/bar"), Some(11));
}
