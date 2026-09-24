use super::*;

#[test]
fn duplicate_cidr_display() {
    let e = IpRadixError::DuplicateCidr {
        cidr: "10.0.0.0/24".to_string(),
        existing_label: "office".to_string(),
        new_label: "corp".to_string(),
    };
    let s = e.to_string();
    assert!(s.contains("10.0.0.0/24"));
    assert!(s.contains("office"));
    assert!(s.contains("corp"));
}

#[test]
fn too_many_groups_display() {
    let e = IpRadixError::TooManyGroups { max_labels: 65534 };
    let s = e.to_string();
    assert!(s.contains("65534"));
}

#[test]
fn existing_variants_still_render() {
    // Spot-checks that all Display arms render correctly.
    let e = IpRadixError::EmptyTree;
    assert_eq!(e.to_string(), "cannot freeze an empty tree");
}
