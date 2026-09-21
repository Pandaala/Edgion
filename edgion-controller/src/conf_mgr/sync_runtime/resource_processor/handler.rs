//! ProcessorHandler trait definition
//!
//! This trait defines resource-specific processing logic that each resource type implements.
//! The handler is stateless and only contains logic - state (cache, workqueue) is in ResourceProcessor.

use kube::Resource;

use super::HandlerContext;

/// Resource processing result
#[derive(Debug)]
pub enum ProcessResult<K> {
    /// Continue processing, resource may have been modified
    Continue(K),
    /// Skip this resource (e.g., filtered, validation failed)
    Skip { reason: String },
}

impl<K> ProcessResult<K> {
    /// Create a Continue result
    pub fn ok(obj: K) -> Self {
        Self::Continue(obj)
    }

    /// Create a Skip result
    pub fn skip(reason: impl Into<String>) -> Self {
        Self::Skip { reason: reason.into() }
    }
}

/// Resource processing handler trait
///
/// This trait defines the resource-specific processing logic.
/// Each resource type implements this trait with its own validation, parsing,
/// and change handling logic.
///
/// The handler is stateless - all state is managed by ResourceProcessor.
#[async_trait::async_trait]
pub trait ProcessorHandler<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// Resource filtering
    ///
    /// Called before processing to determine if the resource should be handled.
    /// Returns true to process, false to skip.
    ///
    /// Default: accept all resources
    fn filter(&self, _obj: &K, _ctx: &HandlerContext) -> bool {
        true
    }

    /// Clean metadata
    ///
    /// Remove unwanted annotations, managedFields, etc.
    /// Called after filter, before validate.
    ///
    /// Default: no-op (metadata cleaning done by processor if config provided)
    fn clean_metadata(&self, _obj: &mut K, _ctx: &HandlerContext) {}

    /// Validate resource
    ///
    /// Perform resource-specific validation (cross-namespace checks, etc.).
    /// Returns a list of warning messages - processing continues even with warnings.
    ///
    /// Default: no validation
    fn validate(&self, _obj: &K, _ctx: &HandlerContext) -> Vec<String> {
        Vec::new()
    }

    /// Preparse resource and return validation errors
    ///
    /// Called by processor after validate(), before parse().
    /// Used for resource-specific preprocessing that may produce validation errors
    /// (e.g., plugin config validation, regex compilation, building runtime structures).
    ///
    /// Errors returned here are automatically merged with validate() errors
    /// and passed to update_status().
    ///
    /// Default: no-op, returns empty errors
    fn preparse(&self, _obj: &mut K, _ctx: &HandlerContext) -> Vec<String> {
        Vec::new()
    }

    /// Parse/preprocess resource
    ///
    /// Perform resource-specific transformations:
    /// - Parse Secret references
    /// - Register to SecretRefManager
    /// - Resolve external references
    ///
    /// Returns ProcessResult::Continue to save, ProcessResult::Skip to discard.
    ///
    /// Default: pass through unchanged
    async fn parse(&self, obj: K, _ctx: &HandlerContext) -> ProcessResult<K> {
        ProcessResult::Continue(obj)
    }

    /// Cleanup on delete
    ///
    /// Called **before** the resource is removed from `ServerCache`. The cached
    /// object is passed in for cleanup (e.g., SecretRefManager registrations).
    /// Lookups via `ServerCache` at this point will still see the old value;
    /// the entry is removed in the next step after this returns.
    ///
    /// Default: no-op
    async fn on_delete(&self, _obj: &K, _ctx: &HandlerContext) {}

    /// Post-change processing
    ///
    /// Called **before** the processed resource is saved to `ServerCache`.
    /// Used for cascading operations (e.g., Secret change triggers Gateway requeue).
    ///
    /// Cross-resource lookups inside `on_change` MUST use `RawStore` (via
    /// `ProcessorObj::get_raw_any`) rather than `ServerCache`, because the
    /// current resource has not yet been written to cache at this point.
    ///
    /// Default: no-op
    async fn on_change(&self, _obj: &K, _ctx: &HandlerContext) {}

    /// Finalize controller-owned transport fields after status derivation but
    /// before the object is atomically published to `ServerCache`.
    ///
    /// This hook is synchronous and must not perform IO. It is intended for
    /// proofs or projections whose input includes fields derived by
    /// `update_status`.
    fn finalize_transport(&self, _obj: &mut K, _ctx: &HandlerContext) {}

    /// Post-publication processing.
    ///
    /// Called only after the final object is visible in `ServerCache`. Use it
    /// for requeues whose consumer must read that exact published generation.
    /// This worker hook may enqueue dependents, but must not perform API IO,
    /// start timers, or sleep; the single per-kind worker must remain serial
    /// and non-blocking.
    async fn after_publish(&self, _obj: &K, _ctx: &HandlerContext) {}

    /// Post-unpublication processing.
    ///
    /// Called only after the deleted object is absent from `ServerCache`.
    /// This worker hook may enqueue dependents, but must not perform API IO,
    /// start timers, or sleep; the single per-kind worker must remain serial
    /// and non-blocking.
    async fn after_unpublish(&self, _obj: &K, _ctx: &HandlerContext) {}

    /// Called when the init LIST phase completes (all resources have been parsed).
    ///
    /// Use this to perform authoritative full-sync operations such as
    /// `replace_all` on global stores, ensuring resources deleted upstream
    /// are cleaned up locally.
    ///
    /// Default: no-op
    fn on_init_done(&self, _ctx: &HandlerContext) {}

    /// Update resource status
    ///
    /// Called after parse() to update the resource's status field.
    /// Handler should set Gateway API standard conditions:
    /// - Accepted: Resource is syntactically and semantically valid
    /// - ResolvedRefs: All references are resolved
    /// - Programmed: Configuration has been sent to the data plane
    /// - Ready: Data plane is ready to serve traffic
    ///
    /// # Arguments
    /// * `obj` - Mutable reference to the resource (handler modifies obj.status)
    /// * `ctx` - Handler context
    /// * `validation_errors` - Errors from validate() method (e.g., ReferenceGrant failures)
    ///
    /// Default: no-op (for resources without status like Secret, Service)
    fn update_status(&self, _obj: &mut K, _ctx: &HandlerContext, _validation_errors: &[String]) {}
}
