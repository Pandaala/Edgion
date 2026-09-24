/// Immutable build-time path precedence, ordered from least to most specific.
///
/// Exact comes first, followed by aggregate static constraints and path depth.
/// Parameter identifiers are capture keys, never specificity inputs. The static
/// counts extend Gateway API precedence to Edgion's named-parameter syntax;
/// they do not prefer a static segment merely because it occurs earlier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PathSpecificity {
    pub is_exact: bool,
    pub static_segment_count: usize,
    /// Unicode characters in static content, including slashes, after `/::`
    /// escapes are resolved. Parameter markers and identifiers contribute zero.
    pub static_char_count: usize,
    pub segment_count: usize,
}

impl PathSpecificity {
    fn new(normalized: &str, is_exact: bool) -> Self {
        let mut specificity = Self {
            is_exact,
            static_segment_count: 0,
            static_char_count: 0,
            segment_count: 0,
        };
        for (index, segment) in normalized.split('/').enumerate() {
            if index > 0 {
                specificity.static_char_count += 1;
            }
            if segment.is_empty() {
                continue;
            }
            specificity.segment_count += 1;
            // Match radix_path::builder::parse_path: only a colon immediately
            // after '/' starts a parameter or escape. An internal ':' is literal.
            let static_segment = if index > 0 && segment.starts_with(':') {
                if let Some(escaped) = segment.strip_prefix("::") {
                    specificity.static_char_count += 1;
                    escaped
                } else {
                    continue;
                }
            } else {
                segment
            };
            specificity.static_segment_count += 1;
            specificity.static_char_count += static_segment.chars().count();
        }
        specificity
    }
}

/// Compiled route path pattern for radix tree matching.
#[derive(Debug)]
pub struct RadixPath {
    /// Normalized path (consecutive slashes merged; non-root Prefix trailing slash removed)
    pub normalized: String,
    /// Path precedence input used only when assigning the snapshot's ranks.
    pub specificity: PathSpecificity,
    /// Whether this is a prefix match (true) or exact match (false)
    pub is_prefix_match: bool,
    /// Index of the associated route
    pub route_idx: usize,
    /// Global precedence rank within the route snapshot (0 = highest priority).
    ///
    /// `RadixPath::new` cannot compute this: the rank depends on every other
    /// path in the same snapshot. It is left at 0 here and filled in by
    /// `RadixRouteMatchEngine::initialize_internal` once all paths are known,
    /// so only read it through a fully built engine.
    pub precedence_rank: u32,
}

impl RadixPath {
    /// Create a new RadixPath from a path pattern.
    ///
    /// # Arguments
    /// * `path` - The path pattern (e.g., "/users/:id/profile")
    /// * `route_idx` - Index of the associated route
    /// * `is_prefix` - Whether this is a prefix match pattern
    pub fn new(path: &str, route_idx: usize, is_prefix: bool) -> Self {
        let is_prefix_match = is_prefix;

        // Normalize path: merge consecutive slashes and validate
        let mut normalized = normalize_path(path);

        // Warn if path was normalized (had consecutive slashes or other issues)
        if normalized != path {
            tracing::warn!(
                "Path normalized: '{}' -> '{}' (consecutive slashes merged)",
                path,
                normalized
            );
        }

        // Validate path starts with '/'
        if !normalized.starts_with('/') && !normalized.is_empty() {
            tracing::warn!("Path should start with '/': '{}'", path);
        }

        // Check for empty parameter names
        if normalized.contains("/:/") || normalized.ends_with("/:") {
            tracing::warn!("Path contains empty parameter name: '{}'", path);
        }

        if is_prefix_match {
            edgion_resources::path_normalization::normalize::canonicalize_path_prefix(&mut normalized);
        }
        let specificity = PathSpecificity::new(&normalized, !is_prefix_match);

        Self {
            normalized,
            specificity,
            is_prefix_match,
            route_idx,
            precedence_rank: 0,
        }
    }

    /// Get the path to use for radix tree insertion.
    /// This returns the normalized path.
    pub fn tree_key(&self) -> &str {
        &self.normalized
    }
}

/// Normalize a path by merging consecutive slashes.
///
/// Examples:
/// - "/api//users" -> "/api/users"
/// - "/api///v1//users/" -> "/api/v1/users/"
/// - "/" -> "/"
fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(path.len());
    let mut prev_was_slash = false;

    for c in path.chars() {
        if c == '/' {
            if !prev_was_slash {
                result.push(c);
            }
            prev_was_slash = true;
        } else {
            result.push(c);
            prev_was_slash = false;
        }
    }

    result
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/match_engine/radix_path_test.rs"]
mod tests;
