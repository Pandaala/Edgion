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

use edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF;
use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;
use edgion_resources::resources::refs::{GrantQuery, ReferenceError};
use edgion_resources::ResourceKind;

use super::super::ref_grant::read_after_registering_cross_ns_dependency;
use super::super::ResourceRef;
use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;

/// A cross-namespace `dns-resolver-ref` that has no matching ReferenceGrant.
pub struct DnsResolverRefDenial {
    pub ref_namespace: String,
    pub name: String,
}

/// Current permission for an already validated actual-owner query.
/// The producer calls this without registering another dependency/worker.
pub(crate) fn dns_query_allowed(query: GrantQuery<'_>, runtime: &ControllerRuntime) -> bool {
    query.source().scope() == query.target().scope()
        || !is_reference_grant_validation_enabled()
        || runtime.ref_grant_store.check_query(query)
}

/// Register the actual raw owner before checking current cross-namespace permission.
pub(crate) fn check_dns_resolver_owner(
    field: DnsResolverOwner<'_>,
    runtime: &ControllerRuntime,
) -> Result<Option<DnsResolverRefDenial>, ReferenceError> {
    let Some(query) = field.query()? else {
        return Ok(None);
    };
    if query.source().scope() == query.target().scope() {
        return Ok(None);
    }
    let namespace = query
        .target()
        .scope()
        .namespace()
        .ok_or(ReferenceError::MissingNamespace)?;
    let kind = match field {
        DnsResolverOwner::Service(_) => ResourceKind::Service,
        DnsResolverOwner::Gateway(_) => ResourceKind::Gateway,
    };
    let owner = ResourceRef::new(
        kind,
        field.raw_namespace().map(str::to_string),
        field.name().unwrap_or_default().to_string(),
    );
    let allowed =
        read_after_registering_cross_ns_dependency(runtime, namespace, &owner, || dns_query_allowed(query, runtime));
    Ok((!allowed).then(|| DnsResolverRefDenial {
        ref_namespace: namespace.to_string(),
        name: query.target().name().to_string(),
    }))
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
