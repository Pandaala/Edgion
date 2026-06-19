//! ReferenceGrant Handler
//!
//! Handles ReferenceGrant resources.
//! This handler maintains the global ReferenceGrantStore and dispatches change events.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::ReferenceGrantChangedEvent;
use super::super::{HandlerContext, ProcessResult, ProcessorHandler};
use edgion_resources::prelude_resources::{ReferenceGrant, ReferenceGrantBeta};

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
        // See `ReferenceGrantHandler::parse` for the rationale behind dispatching
        // for orphaned namespaces (old - new) BEFORE the upsert overwrites the
        // cached old grant. Without this, removing a from/to namespace from a
        // grant would leave stale `ResolvedRefs=True` on the orphaned routes.
        let v1: ReferenceGrant = rg.clone().into();
        let key = ReferenceGrantHandler::build_key(&v1);
        let store = &_ctx.runtime().ref_grant_store;

        let new_affected = ReferenceGrantHandler::identify_affected_namespaces(&v1);
        let orphaned_namespaces: HashSet<String> = match store.get(&key) {
            Some(old) => ReferenceGrantHandler::identify_affected_namespaces(&old)
                .difference(&new_affected)
                .cloned()
                .collect(),
            None => HashSet::new(),
        };

        store.upsert(key.clone(), Arc::new(v1));

        tracing::debug!(
            component = "reference_grant_handler",
            key = %key,
            version = "v1beta1",
            orphaned_ns_count = orphaned_namespaces.len(),
            "Updated ReferenceGrant (v1beta1→v1) in store"
        );

        if !orphaned_namespaces.is_empty() {
            tracing::info!(
                component = "reference_grant_handler",
                key = %key,
                version = "v1beta1",
                orphaned_ns_count = orphaned_namespaces.len(),
                "ReferenceGrant (v1beta1) updated, dispatching event for orphaned namespaces"
            );
            ReferenceGrantHandler::dispatch_change_event(orphaned_namespaces, _ctx).await;
        }

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

        ReferenceGrantHandler::dispatch_change_event(affected_namespaces, _ctx).await;
    }

    async fn on_delete(&self, rg: &ReferenceGrantBeta, _ctx: &HandlerContext) {
        let v1: ReferenceGrant = rg.clone().into();
        let key = ReferenceGrantHandler::build_key(&v1);
        let store = &_ctx.runtime().ref_grant_store;

        let affected_namespaces = ReferenceGrantHandler::identify_affected_namespaces(&v1);

        store.remove(&key);

        tracing::info!(
            component = "reference_grant_handler",
            key = %key,
            version = "v1beta1",
            affected_ns_count = affected_namespaces.len(),
            "ReferenceGrant (v1beta1) deleted, dispatching event"
        );

        ReferenceGrantHandler::dispatch_change_event(affected_namespaces, _ctx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::{
        ReferenceGrantChangedEvent, RevalidationListener,
    };
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::{
        ReferenceGrantBeta, ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use parking_lot::Mutex;

    /// Recording listener that captures every dispatched event in order.
    struct RecordingListener {
        events: Mutex<Vec<HashSet<String>>>,
    }

    impl RecordingListener {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn snapshot(&self) -> Vec<HashSet<String>> {
            self.events.lock().clone()
        }
    }

    #[async_trait::async_trait]
    impl RevalidationListener for RecordingListener {
        async fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent) {
            self.events.lock().push(event.affected_namespaces.clone());
        }
    }

    fn build_grant(namespace: &str, name: &str, from_namespaces: &[&str]) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: from_namespaces
                    .iter()
                    .map(|ns| ReferenceGrantFrom {
                        group: "gateway.networking.k8s.io".to_string(),
                        kind: "HTTPRoute".to_string(),
                        namespace: (*ns).to_string(),
                    })
                    .collect(),
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: None,
                }],
            },
        }
    }

    fn build_grant_beta(namespace: &str, name: &str, from_namespaces: &[&str]) -> ReferenceGrantBeta {
        ReferenceGrantBeta {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantBetaSpec {
                from: from_namespaces
                    .iter()
                    .map(|ns| ReferenceGrantFrom {
                        group: "gateway.networking.k8s.io".to_string(),
                        kind: "HTTPRoute".to_string(),
                        namespace: (*ns).to_string(),
                    })
                    .collect(),
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: None,
                }],
            },
        }
    }

    fn make_ctx(runtime: Arc<ControllerRuntime>) -> HandlerContext {
        HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            8,  // max_trigger_cycles
            32, // max_trigger_depth
            runtime,
        )
    }

    /// Union of all events' namespaces — useful when assertion does not care
    /// whether dispatch happened in one event or two.
    fn union_dispatched(events: &[HashSet<String>]) -> HashSet<String> {
        let mut acc: HashSet<String> = HashSet::new();
        for ev in events {
            acc.extend(ev.iter().cloned());
        }
        acc
    }

    /// Regression: when a ReferenceGrant changes its `spec.from[*].namespace`,
    /// the OLD `from` namespace must be re-dispatched so its routes get
    /// revalidated. Since `parse()` overwrites the cache before `on_change()`
    /// fires, the orphan dispatch must happen inside `parse()` itself.
    #[tokio::test]
    async fn parse_dispatches_orphaned_from_namespace_on_update() {
        let runtime = Arc::new(ControllerRuntime::default());
        let listener = RecordingListener::new();
        runtime.ref_grant_dispatcher.register_listener(listener.clone());

        let ctx = make_ctx(runtime.clone());
        let handler = ReferenceGrantHandler::new();

        // Initial grant: from=app-a, to(=ns)=backend
        let grant_v1 = build_grant("backend", "g1", &["app-a"]);
        match handler.parse(grant_v1.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }

        // Update grant: from=app-b (replaces app-a), to(=ns)=backend
        let grant_v2 = build_grant("backend", "g1", &["app-b"]);
        match handler.parse(grant_v2.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }

        let events = listener.snapshot();
        assert!(
            events.len() >= 3,
            "expected at least 3 dispatches (initial + orphan-of-update + new-of-update), got {}: {:?}",
            events.len(),
            events
        );

        // The events emitted DURING the update (everything after the initial create)
        // must collectively cover BOTH `app-a` (orphaned) AND `app-b` (new).
        let update_events = &events[1..];
        let dispatched = union_dispatched(update_events);

        assert!(
            dispatched.contains("app-a"),
            "OLD from-namespace `app-a` must be re-dispatched on update; got {:?}",
            dispatched
        );
        assert!(
            dispatched.contains("app-b"),
            "NEW from-namespace `app-b` must be dispatched on update; got {:?}",
            dispatched
        );
        assert!(
            dispatched.contains("backend"),
            "to-namespace `backend` must be dispatched on update; got {:?}",
            dispatched
        );
    }

    /// Steady-state: if the spec is unchanged across updates, only the
    /// on_change dispatch fires — no orphan dispatch is emitted because
    /// `old - new` is empty.
    #[tokio::test]
    async fn parse_does_not_dispatch_orphan_when_namespaces_unchanged() {
        let runtime = Arc::new(ControllerRuntime::default());
        let listener = RecordingListener::new();
        runtime.ref_grant_dispatcher.register_listener(listener.clone());

        let ctx = make_ctx(runtime.clone());
        let handler = ReferenceGrantHandler::new();

        let grant = build_grant("backend", "g1", &["app-a"]);
        match handler.parse(grant.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }
        let events_after_first = listener.snapshot().len();

        // Re-apply the same grant (no namespace change).
        match handler.parse(grant.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }
        let events_after_second = listener.snapshot().len();

        // Second update should add exactly one event (the on_change),
        // NOT two (would mean we also fired an orphan event).
        let added = events_after_second - events_after_first;
        assert_eq!(
            added, 1,
            "expected exactly 1 dispatch on idempotent update (on_change only), got {}",
            added
        );
    }

    /// First create has no cached old grant — must not synthesize an orphan
    /// dispatch from a non-existent old state.
    #[tokio::test]
    async fn parse_first_create_emits_no_orphan_dispatch() {
        let runtime = Arc::new(ControllerRuntime::default());
        let listener = RecordingListener::new();
        runtime.ref_grant_dispatcher.register_listener(listener.clone());

        let ctx = make_ctx(runtime.clone());
        let handler = ReferenceGrantHandler::new();

        let grant = build_grant("backend", "g1", &["app-a"]);
        match handler.parse(grant.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }

        assert_eq!(listener.snapshot().len(), 1);
    }

    /// v1beta1 handler must apply the same orphan-on-update behavior.
    #[tokio::test]
    async fn beta_parse_dispatches_orphaned_from_namespace_on_update() {
        let runtime = Arc::new(ControllerRuntime::default());
        let listener = RecordingListener::new();
        runtime.ref_grant_dispatcher.register_listener(listener.clone());

        let ctx = make_ctx(runtime.clone());
        let handler = ReferenceGrantBetaHandler::new();

        let grant_v1 = build_grant_beta("backend", "g1", &["app-a"]);
        match handler.parse(grant_v1.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }

        let grant_v2 = build_grant_beta("backend", "g1", &["app-b"]);
        match handler.parse(grant_v2.clone(), &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }

        let events = listener.snapshot();
        assert!(events.len() >= 3, "expected at least 3 dispatches, got {:?}", events);

        let update_events = &events[1..];
        let dispatched = union_dispatched(update_events);

        assert!(
            dispatched.contains("app-a"),
            "OLD from-namespace `app-a` must be re-dispatched on v1beta1 update; got {:?}",
            dispatched
        );
        assert!(
            dispatched.contains("app-b"),
            "NEW from-namespace `app-b` must be dispatched on v1beta1 update; got {:?}",
            dispatched
        );
    }
}
