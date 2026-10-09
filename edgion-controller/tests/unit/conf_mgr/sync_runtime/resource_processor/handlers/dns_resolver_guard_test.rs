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

// Preserve the original fixture call shape using actual typed owners only in tests.
fn check_dns_resolver_ref_grant(
    from_ns: &str,
    from_group: &str,
    from_kind: &str,
    annotations: Option<&BTreeMap<String, String>>,
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
) -> Option<DnsResolverRefDenial> {
    assert_eq!((from_group, from_kind), ("", "Service"));
    let service = k8s_openapi::api::core::v1::Service {
        metadata: kube::api::ObjectMeta {
            name: Some(owner.name.clone()),
            namespace: Some(from_ns.into()),
            annotations: annotations.cloned(),
            ..Default::default()
        },
        ..Default::default()
    };
    super::check_dns_resolver_owner(DnsResolverOwner::Service(&service), runtime).unwrap()
}

#[test]
fn owning_dns_permission_switch_never_exempts_invalid_source_or_target() {
    const CHILD: &str = "EDGION_R6A_DNS_VALIDATION_OFF_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard::tests::owning_dns_permission_switch_never_exempts_invalid_source_or_target","--nocapture"]).env(CHILD,"1").output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let mut config = crate::config::EdgionControllerConfig::default();
    config.validation.enable_reference_grant_validation = false;
    crate::config::init_controller_config(config);
    let runtime = ControllerRuntime::default();
    let mut source = k8s_openapi::api::core::v1::Service {
        metadata: kube::api::ObjectMeta {
            name: Some("source".into()),
            annotations: Some(ann("dns/resolver")),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        super::check_dns_resolver_owner(DnsResolverOwner::Service(&source), &runtime)
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns").len(), 1);
    runtime
        .cross_ns_ref_mgr
        .clear_value_refs(&ResourceRef::new(ResourceKind::Service, None, "source".into()));
    source.metadata.namespace = Some("".into());
    assert!(super::check_dns_resolver_owner(DnsResolverOwner::Service(&source), &runtime).is_err());
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
    source.metadata.namespace = None;
    source.metadata.annotations = Some(ann("dns/a/b"));
    assert!(
        super::check_dns_resolver_owner(DnsResolverOwner::Service(&source), &runtime)
            .unwrap()
            .is_none()
    );
    assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
}
