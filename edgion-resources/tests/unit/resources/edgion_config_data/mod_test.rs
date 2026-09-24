use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

#[test]
fn iplist_with_duplicate_cidr_is_partial_not_pre_parse_error() {
    use crate::matcher::ip_cidr_helpers::collect_entry_errors;
    let items = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
    ];
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: ConfigDataVisibility::Namespace,
        data: ConfigEntry::IpList(IpListData { items: items.clone() }),
        unknown_fields: Vec::new(),
    };
    let ed = EdgionConfigData {
        metadata: ObjectMeta {
            name: Some("dup".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    // Partial-good: no pre_parse error any more.
    assert!(ed.validate_pre_parse().is_empty());
    // The duplicate is surfaced by collect_entry_errors (drives PartiallyInvalid).
    let errs = collect_entry_errors(&items);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].reason.contains("duplicate CIDR"));
}

#[test]
fn iplist_host_bit_duplicate_is_partial_not_pre_parse_error() {
    // 10.0.0.0/8 and 10.0.0.1/8 normalize to the same /8 prefix in the radix builder.
    let items = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["10.0.0.1/8".into()],
        },
    ];
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: ConfigDataVisibility::Namespace,
        data: ConfigEntry::IpList(IpListData { items: items.clone() }),
        unknown_fields: Vec::new(),
    };
    let ed = EdgionConfigData {
        metadata: ObjectMeta {
            name: Some("host-bit-dup".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    assert!(ed.validate_pre_parse().is_empty());
    // Note: collect_entry_errors compares normalized CIDR strings; 10.0.0.1/8 vs 10.0.0.0/8
    // normalize differently as strings, so this case is caught at radix build() / index time,
    // not here. Assert at least that pre_parse no longer rejects it.
}
