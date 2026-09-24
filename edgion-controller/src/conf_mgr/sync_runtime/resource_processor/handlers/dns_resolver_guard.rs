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

use super::super::ref_grant::read_after_registering_cross_ns_dependency;
use super::super::ResourceRef;
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
/// when grant validation is disabled. A valid cross-namespace ref is registered
/// before the validation-mode or authorization read. `from_group`/`from_kind`
/// identify the referrer (`"gateway.networking.k8s.io"`/`"Gateway"` or
/// `""`/`"Service"`).
pub fn check_dns_resolver_ref_grant(
    from_ns: &str,
    from_group: &str,
    from_kind: &str,
    annotations: Option<&BTreeMap<String, String>>,
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
) -> Option<DnsResolverRefDenial> {
    let value = annotations?.get(DNS_RESOLVER_REF)?;
    let key = LinkSysKey::parse(value)?;
    if key.namespace == from_ns {
        return None;
    }
    let allowed = read_after_registering_cross_ns_dependency(runtime, &key.namespace, owner, || {
        !is_reference_grant_validation_enabled()
            || runtime.ref_grant_store.check_reference_allowed(
                from_ns,
                from_group,
                from_kind,
                &key.namespace,
                LINK_SYS_GROUP,
                LINK_SYS_KIND,
                Some(&key.name),
            )
    });
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/dns_resolver_guard_test.rs"]
mod tests;
