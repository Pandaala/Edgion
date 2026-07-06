//! Cross-namespace ReferenceGrant gate for the `edgion.io/dns-resolver-ref`
//! annotation (Service / Gateway → an httpdns `edgion.io/LinkSys`).
//!
//! Controller-YAML / EdgionGatewayConfig resolver refs are operator-owned and
//! trusted (no grant); only the per-Service / per-Gateway annotation refs are
//! tenant-owned and gated here. A cross-namespace ref lacking a matching
//! ReferenceGrant is (1) reported by the caller and (2) stripped from the object
//! before it ships to the data plane, so the gateway never honors an
//! unpermitted reference.

use std::collections::BTreeMap;

use edgion_common::dns::resolver_select::LinkSysKey;
use edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF;

use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;

/// A cross-namespace `dns-resolver-ref` that has no matching ReferenceGrant.
pub struct DnsResolverRefDenial {
    pub ref_namespace: String,
    pub name: String,
}

/// API group of a `dns-resolver-ref` target (an httpdns LinkSys).
const LINK_SYS_GROUP: &str = "edgion.io";
/// Kind of a `dns-resolver-ref` target.
const LINK_SYS_KIND: &str = "LinkSys";

/// Check a resource's `dns-resolver-ref` annotation for a cross-namespace
/// httpdns LinkSys reference that lacks a ReferenceGrant.
///
/// Returns `None` for a same-namespace ref, an absent / malformed annotation, or
/// when grant validation is disabled. `from_group`/`from_kind` identify the
/// referrer (`"gateway.networking.k8s.io"`/`"Gateway"` or `""`/`"Service"`).
pub fn check_dns_resolver_ref_grant(
    from_ns: &str,
    from_group: &str,
    from_kind: &str,
    annotations: Option<&BTreeMap<String, String>>,
    runtime: &ControllerRuntime,
) -> Option<DnsResolverRefDenial> {
    if !is_reference_grant_validation_enabled() {
        return None;
    }
    let value = annotations?.get(DNS_RESOLVER_REF)?;
    let key = LinkSysKey::parse(value)?;
    if key.namespace == from_ns {
        return None;
    }
    let allowed = runtime.ref_grant_store.check_reference_allowed(
        from_ns,
        from_group,
        from_kind,
        &key.namespace,
        LINK_SYS_GROUP,
        LINK_SYS_KIND,
        Some(&key.name),
    );
    if allowed {
        None
    } else {
        Some(DnsResolverRefDenial {
            ref_namespace: key.namespace,
            name: key.name,
        })
    }
}

/// Remove the `dns-resolver-ref` annotation in place. Called after a denial so
/// the data plane never honors an unpermitted cross-namespace reference. Only
/// the shipped/cached copy is mutated — the source-of-truth object in the
/// apiserver keeps the annotation, so a later ReferenceGrant re-enables it on
/// the next reconcile.
pub fn strip_dns_resolver_ref(annotations: &mut Option<BTreeMap<String, String>>) {
    if let Some(map) = annotations {
        map.remove(DNS_RESOLVER_REF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::ControllerRuntime;

    fn ann(reference: &str) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(DNS_RESOLVER_REF.to_string(), reference.to_string());
        m
    }

    #[test]
    fn absent_annotation_is_not_denied() {
        let runtime = ControllerRuntime::default();
        assert!(check_dns_resolver_ref_grant("app", "", "Service", None, &runtime).is_none());
        let empty = BTreeMap::new();
        assert!(check_dns_resolver_ref_grant("app", "", "Service", Some(&empty), &runtime).is_none());
    }

    #[test]
    fn same_namespace_ref_is_not_denied() {
        let runtime = ControllerRuntime::default();
        let a = ann("app/aliyun");
        assert!(check_dns_resolver_ref_grant("app", "", "Service", Some(&a), &runtime).is_none());
    }

    #[test]
    fn cross_namespace_ref_without_grant_is_denied() {
        // Empty ref_grant_store + default (enabled) validation → cross-ns denied.
        let runtime = ControllerRuntime::default();
        let a = ann("dns/aliyun");
        let denial = check_dns_resolver_ref_grant("app", "", "Service", Some(&a), &runtime);
        let denial = denial.expect("cross-namespace ref without grant must be denied");
        assert_eq!(denial.ref_namespace, "dns");
        assert_eq!(denial.name, "aliyun");
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
}
