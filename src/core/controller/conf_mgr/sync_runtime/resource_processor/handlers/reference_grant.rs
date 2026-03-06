//! ReferenceGrant Handler
//!
//! Handles ReferenceGrant resources.
//! This handler maintains the global ReferenceGrantStore and dispatches change events.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{get_global_dispatcher, get_global_reference_grant_store, ReferenceGrantChangedEvent};
use super::super::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::ReferenceGrant;

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
    fn build_key(rg: &ReferenceGrant) -> String {
        let namespace = rg.metadata.namespace.as_deref().unwrap_or("default");
        let name = rg.metadata.name.as_deref().unwrap_or("");
        format!("{}/{}", namespace, name)
    }

    /// Identify affected namespaces from a ReferenceGrant
    /// Returns both the grant's namespace (to) and from namespaces
    fn identify_affected_namespaces(rg: &ReferenceGrant) -> HashSet<String> {
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
    fn dispatch_change_event(affected_namespaces: HashSet<String>) {
        if affected_namespaces.is_empty() {
            return;
        }

        let dispatcher = get_global_dispatcher();
        let event = ReferenceGrantChangedEvent { affected_namespaces };
        dispatcher.dispatch(&event);
    }
}

impl Default for ReferenceGrantHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<ReferenceGrant> for ReferenceGrantHandler {
    fn parse(&self, rg: ReferenceGrant, _ctx: &HandlerContext) -> ProcessResult<ReferenceGrant> {
        // Parse is called during both add and update
        // We update the store here
        let key = Self::build_key(&rg);
        let store = get_global_reference_grant_store();

        // Update the store
        store.upsert(key.clone(), Arc::new(rg.clone()));

        tracing::debug!(
            component = "reference_grant_handler",
            key = %key,
            "Updated ReferenceGrant in store"
        );

        ProcessResult::Continue(rg)
    }

    fn on_change(&self, obj: &ReferenceGrant, _ctx: &HandlerContext) {
        // Collect affected namespaces from the grant
        let affected_namespaces = Self::identify_affected_namespaces(obj);

        let key = Self::build_key(obj);
        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant changed, dispatching event"
        );

        // Dispatch change event to trigger revalidation of affected resources
        Self::dispatch_change_event(affected_namespaces);
    }

    fn on_delete(&self, rg: &ReferenceGrant, _ctx: &HandlerContext) {
        let key = Self::build_key(rg);
        let store = get_global_reference_grant_store();

        // Collect affected namespaces before removing
        let affected_namespaces = Self::identify_affected_namespaces(rg);

        // Remove from store
        store.remove(&key);

        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant deleted, dispatching event"
        );

        // Dispatch change event to trigger revalidation
        Self::dispatch_change_event(affected_namespaces);
    }
}
