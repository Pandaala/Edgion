use super::*;
use crate::matcher::ip_cidr_helpers::validate_groups_structure;

#[test]
fn empty_items_validation_errs() {
    let data = IpListData { items: vec![] };
    assert!(validate_groups_structure(&data.items).is_err());
}

#[test]
fn single_group_valid() {
    let data = IpListData {
        items: vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }],
    };
    assert!(validate_groups_structure(&data.items).is_ok());
}
