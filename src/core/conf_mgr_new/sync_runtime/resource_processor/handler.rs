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
    fn filter(&self, _obj: &K) -> bool {
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
    fn parse(&self, obj: K, _ctx: &HandlerContext) -> ProcessResult<K> {
        ProcessResult::Continue(obj)
    }

    /// Cleanup on delete
    ///
    /// Called when a resource is deleted.
    /// Used to clean up SecretRefManager registrations, etc.
    ///
    /// Default: no-op
    fn on_delete(&self, _obj: &K, _ctx: &HandlerContext) {}

    /// Post-change processing
    ///
    /// Called after a resource is saved to cache.
    /// Used for cascading operations (e.g., Secret change triggers Gateway requeue).
    ///
    /// Default: no-op
    fn on_change(&self, _obj: &K, _ctx: &HandlerContext) {}
}

/// Default handler that passes through all resources unchanged
///
/// Useful for resources that don't need special processing.
pub struct DefaultHandler;

impl<K> ProcessorHandler<K> for DefaultHandler
where
    K: Resource + Clone + Send + Sync + 'static,
{
    // All methods use default implementations
}
