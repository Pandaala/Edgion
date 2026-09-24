use super::*;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::ResourceKind;

fn ann(reference: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(DNS_RESOLVER_REF.to_string(), reference.to_string());
    m
}

fn owner() -> ResourceRef {
    ResourceRef::new(ResourceKind::Service, Some("app".to_string()), "backend".to_string())
}

#[test]
fn absent_annotation_is_not_denied() {
    let runtime = ControllerRuntime::default();
    let owner = owner();
    assert!(check_dns_resolver_ref_grant("app", "", "Service", None, &owner, &runtime).is_none());
    let empty = BTreeMap::new();
    assert!(check_dns_resolver_ref_grant("app", "", "Service", Some(&empty), &owner, &runtime).is_none());
}

#[test]
fn same_namespace_ref_is_not_denied() {
    let runtime = ControllerRuntime::default();
    let a = ann("app/aliyun");
    assert!(check_dns_resolver_ref_grant("app", "", "Service", Some(&a), &owner(), &runtime).is_none());
}

#[test]
fn cross_namespace_ref_without_grant_is_denied() {
    // Empty ref_grant_store + default (enabled) validation → cross-ns denied.
    let runtime = ControllerRuntime::default();
    let a = ann("dns/aliyun");
    let owner = owner();
    let denial = check_dns_resolver_ref_grant("app", "", "Service", Some(&a), &owner, &runtime);
    let denial = denial.expect("cross-namespace ref without grant must be denied");
    assert_eq!(denial.ref_namespace, "dns");
    assert_eq!(denial.name, "aliyun");
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns"), vec![owner]);
}

#[test]
fn strip_removes_only_the_dns_resolver_ref_key() {
    let mut annotations = Some({
        let mut m = ann("dns/aliyun");
        m.insert("edgion.io/other".to_string(), "keep".to_string());
        m
    });
    strip_dns_resolver_ref(&mut annotations);
    let m = annotations.unwrap();
    assert!(!m.contains_key(DNS_RESOLVER_REF));
    assert_eq!(m.get("edgion.io/other").map(String::as_str), Some("keep"));
}
