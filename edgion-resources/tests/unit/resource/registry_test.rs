use super::*;

#[test]
fn test_resource_registry() {
    let all_names = all_resource_type_names();
    assert!(all_names.contains(&"routes"));
    assert!(all_names.contains(&"services"));
}
