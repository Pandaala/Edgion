//! ReferenceGrant Handler
//!
//! Handles ReferenceGrant resources.
//! This handler maintains the global ReferenceGrantStore and dispatches change events.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{get_global_dispatcher, get_global_reference_grant_store, ReferenceGrantChangedEvent};
use super::super::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::{ReferenceGrant, ReferenceGrantBeta};

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
    pub(crate) async fn dispatch_change_event(affected_namespaces: HashSet<String>) {
        if affected_namespaces.is_empty() {
            return;
        }

        let dispatcher = get_global_dispatcher();
        let event = ReferenceGrantChangedEvent { affected_namespaces };
        dispatcher.dispatch(&event).await;
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

    async fn on_change(&self, obj: &ReferenceGrant, _ctx: &HandlerContext) {
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
        Self::dispatch_change_event(affected_namespaces).await;
    }

    async fn on_delete(&self, rg: &ReferenceGrant, _ctx: &HandlerContext) {
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
        Self::dispatch_change_event(affected_namespaces).await;
    }
}

// ---------------------------------------------------------------------------
// v1beta1 compat handler — converts to v1 and delegates to the same store
// ---------------------------------------------------------------------------

/// ReferenceGrant v1beta1 handler
///
/// Converts v1beta1 resources to v1 before delegating to the same
/// ReferenceGrantStore and event dispatcher used by the v1 handler.
pub struct ReferenceGrantBetaHandler;

impl ReferenceGrantBetaHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReferenceGrantBetaHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<ReferenceGrantBeta> for ReferenceGrantBetaHandler {
    async fn parse(&self, rg: ReferenceGrantBeta, _ctx: &HandlerContext) -> ProcessResult<ReferenceGrantBeta> {
        let v1: ReferenceGrant = rg.clone().into();
        let key = ReferenceGrantHandler::build_key(&v1);
        let store = get_global_reference_grant_store();

        store.upsert(key.clone(), Arc::new(v1));

        tracing::debug!(
            component = "reference_grant_handler",
            key = %key,
            version = "v1beta1",
            "Updated ReferenceGrant (v1beta1→v1) in store"
        );

        ProcessResult::Continue(rg)
    }

    async fn on_change(&self, obj: &ReferenceGrantBeta, _ctx: &HandlerContext) {
        let v1: ReferenceGrant = obj.clone().into();
        let affected_namespaces = ReferenceGrantHandler::identify_affected_namespaces(&v1);

        let key = ReferenceGrantHandler::build_key(&v1);
        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            version = "v1beta1",
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant (v1beta1) changed, dispatching event"
        );

        ReferenceGrantHandler::dispatch_change_event(affected_namespaces).await;
    }

    async fn on_delete(&self, rg: &ReferenceGrantBeta, _ctx: &HandlerContext) {
        let v1: ReferenceGrant = rg.clone().into();
        let key = ReferenceGrantHandler::build_key(&v1);
        let store = get_global_reference_grant_store();

        let affected_namespaces = ReferenceGrantHandler::identify_affected_namespaces(&v1);

        store.remove(&key);

        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            version = "v1beta1",
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant (v1beta1) deleted, dispatching event"
        );

        ReferenceGrantHandler::dispatch_change_event(affected_namespaces).await;
    }
}
