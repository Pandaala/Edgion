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

    /// Identify the namespaces whose dependents a ReferenceGrant change can affect.
    ///
    /// A grant only authorizes references whose target lives in the grant's own
    /// namespace, and `CrossNsRefManager` keys every dependency by that target
    /// namespace. The `from` namespaces are therefore not dependency keys:
    /// dispatching them would requeue unrelated resources that reference into
    /// those namespaces. Owners in old or new `from` namespaces are all
    /// registered under the grant namespace, so a `from` change is covered too.
    pub(crate) fn identify_affected_namespaces(rg: &ReferenceGrant) -> HashSet<String> {
        rg.metadata.namespace.iter().cloned().collect()
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
        // Store before publication so on_change revalidation reads the new grant.
        let key = Self::build_key(&rg);
        _ctx.runtime().ref_grant_store.upsert(key.clone(), Arc::new(rg.clone()));

        tracing::debug!(
            component = "reference_grant_handler",
            key = %key,
            "Updated ReferenceGrant in store"
        );

        ProcessResult::Continue(rg)
    }

    async fn on_change(&self, obj: &ReferenceGrant, _ctx: &HandlerContext) {
        // The grant namespace covers dependents of both the old and new
        // generation, because the namespace is part of the grant key.
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
