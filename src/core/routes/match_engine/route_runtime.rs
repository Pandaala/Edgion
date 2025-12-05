use crate::types::err::EdError;
use pingora_proxy::Session;

/// Trait for route entry that can be used with RadixRouteMatchEngine
///
/// This trait defines the minimal interface needed for route matching:
/// - Path extraction for radix tree indexing
/// - Identity for logging and debugging
/// - Deep matching for additional validation (headers, methods, etc.)
pub trait RouteEntry: Send + Sync {
    /// Extract all path patterns from this route with their match_engine types
    /// Returns Vec<(path, is_prefix)>
    fn extract_paths(&self) -> Vec<(String, bool)>;

    /// Get a unique identifier for this route (for logging)
    fn identifier(&self) -> String;

    /// Perform deep match_engine validation (headers, methods, query params, etc.)
    fn deep_match(&self, session: &Session) -> Result<bool, EdError>;
}
