//! ReferenceGrant Handler
//!
//! Handles ReferenceGrant resources.
//! This handler maintains the global ReferenceGrantStore and dispatches change events.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::ReferenceGrantChangedEvent;
use super::super::{HandlerContext, ProcessResult, ProcessorHandler};
use edgion_resources::prelude_resources::ReferenceGrant;

/// ReferenceGrant handler
///
/// Maintains the global ReferenceGrantStore and dispatches change events
/// when ReferenceGrants are created, updated, or deleted.
pub struct ReferenceGrantHandler;

impl ReferenceGrantHandler {
    pub fn new() -> Self {
        Self
    }

    /// Build the key for a ReferenceGrant (namespace/name)
    pub(crate) fn build_key(rg: &ReferenceGrant) -> String {
        let namespace = rg.metadata.namespace.as_deref().unwrap_or("default");
        let name = rg.metadata.name.as_deref().unwrap_or("");
        format!("{}/{}", namespace, name)
    }

    /// Identify affected namespaces from a ReferenceGrant
    /// Returns both the grant's namespace (to) and from namespaces
    pub(crate) fn identify_affected_namespaces(rg: &ReferenceGrant) -> HashSet<String> {
        let mut affected = HashSet::new();

        // The namespace where the grant is defined (to_namespace)
        if let Some(ns) = rg.metadata.namespace.as_ref() {
            affected.insert(ns.clone());
        }

        // All from namespaces
        for from in &rg.spec.from {
            affected.insert(from.namespace.clone());
        }

        affected
    }

    /// Dispatch change event to listeners
    pub(crate) async fn dispatch_change_event(affected_namespaces: HashSet<String>, ctx: &HandlerContext) {
        if affected_namespaces.is_empty() {
            return;
        }

        let event = ReferenceGrantChangedEvent { affected_namespaces };
        ctx.runtime().ref_grant_dispatcher.dispatch(&event).await;
    }
}

impl Default for ReferenceGrantHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<ReferenceGrant> for ReferenceGrantHandler {
    async fn parse(&self, rg: ReferenceGrant, _ctx: &HandlerContext) -> ProcessResult<ReferenceGrant> {
        // Bug guard: if a ReferenceGrant's `spec.from[*].namespace` (or
        // `spec.to.namespace` via metadata.namespace) changes, routes that
        // were permitted by the OLD grant must also be revalidated. The
        // `on_change` callback only sees the NEW grant, so namespaces that
        // disappear from the grant would otherwise keep stale
        // `ResolvedRefs=True` until something else triggers them.
        //
        // We compute `old - new` here (before upsert overwrites the cache)
        // and dispatch a revalidation for those orphaned namespaces. The
        // subsequent `on_change` dispatches for the NEW grant's namespaces.
        // In steady state (namespaces unchanged) we send only the on_change
        // event; downstream listeners are idempotent under duplicates.
        let key = Self::build_key(&rg);
        let store = &_ctx.runtime().ref_grant_store;

        // Capture orphaned namespaces (in old but not in new) before the upsert.
        // Note: `get` then `upsert` is not atomic at the store level. We rely on
        // the FileSystemResourceController processing one ReferenceGrant key at a
        // time on its receive loop, so no other parse() runs against the same
        // key concurrently. `ControllerRuntime::reset()` (rare, leader-change
        // only) is the only other writer; if it interleaves, some orphaned
        // namespaces may be missed on this pass — they will be re-dispatched
        // when the next reset reprocesses all ReferenceGrants.
        let new_affected = Self::identify_affected_namespaces(&rg);
        let orphaned_namespaces: HashSet<String> = match store.get(&key) {
            Some(old) => Self::identify_affected_namespaces(&old)
                .difference(&new_affected)
                .cloned()
                .collect(),
            None => HashSet::new(),
        };

        store.upsert(key.clone(), Arc::new(rg.clone()));

        tracing::debug!(
            component = "reference_grant_handler",
            key = %key,
            orphaned_ns_count = orphaned_namespaces.len(),
            "Updated ReferenceGrant in store"
        );

        if !orphaned_namespaces.is_empty() {
            tracing::info!(
                component = "reference_grant_handler",
                key = %key,
                orphaned_ns_count = orphaned_namespaces.len(),
                "ReferenceGrant updated, dispatching event for orphaned namespaces"
            );
            Self::dispatch_change_event(orphaned_namespaces, _ctx).await;
        }

        ProcessResult::Continue(rg)
    }

    async fn on_change(&self, obj: &ReferenceGrant, _ctx: &HandlerContext) {
        // Collect affected namespaces from the (NEW) grant.
        // Orphaned namespaces from a changed grant were already dispatched
        // in `parse()` before the cache was overwritten.
        let affected_namespaces = Self::identify_affected_namespaces(obj);

        let key = Self::build_key(obj);
        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant changed, dispatching event"
        );

        Self::dispatch_change_event(affected_namespaces, _ctx).await;
    }

    async fn on_delete(&self, rg: &ReferenceGrant, _ctx: &HandlerContext) {
        let key = Self::build_key(rg);
        let store = &_ctx.runtime().ref_grant_store;

        let affected_namespaces = Self::identify_affected_namespaces(rg);
        store.remove(&key);

        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant deleted, dispatching event"
        );

        Self::dispatch_change_event(affected_namespaces, _ctx).await;
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant_test.rs"]
mod tests;
