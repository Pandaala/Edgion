use std::sync::Arc;

/// `RadixHost` represents a hostname pattern used in the radix-tree based matcher.
/// The current algorithm relies on reversing hostnames to make longest-prefix
/// comparison straightforward:
/// - `"api.example.com"` becomes `"com.example.api"`
/// - `"*.example.com"` becomes `"com.example"` for prefix matching
/// - Multi-level wildcards (e.g. `*.*.example.com`) are matched by counting the
///   leading wildcard segments
///
/// Note: if the matching strategy ever changes (e.g. DFA/NFA/regex), please
/// update this documentation accordingly.
pub struct RadixHost<T> {
    pub original: String,
    pub radix_key: String,
    pub is_wildcard: bool,
    pub wildcard_count: usize,
    pub runtime: Arc<T>,
}

// Manual Clone implementation: Arc<T> is always cloneable (shallow copy)
impl<T> Clone for RadixHost<T> {
    fn clone(&self) -> Self {
        Self {
            original: self.original.clone(),
            radix_key: self.radix_key.clone(),
            is_wildcard: self.is_wildcard,
            wildcard_count: self.wildcard_count,
            runtime: Arc::clone(&self.runtime), // Shallow copy of Arc
        }
    }
}

impl<T> RadixHost<T> {
    /// Create a `RadixHost` from a hostname pattern.
    ///
    /// - `host`: plain hostname or wildcard pattern (e.g. `"example.com"`,
    ///   `"*.example.com"`)
    /// - `runtime`: runtime data associated with the host (typically a route or
    ///   service handle)
    ///
    /// Only leading `*.` wildcards are supported. We record how many wildcard
    /// segments are present so that matching can enforce the same depth.
    ///
    /// # Examples
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use crate::matcher::host_match::radix_match::radix_host::RadixHost;
    ///
    /// let runtime = Arc::new(());
    ///
    /// let host = RadixHost::new("api.example.com", runtime.clone());
    /// assert_eq!(host.radix_key, "com.example.api");
    /// assert!(!host.is_wildcard);
    ///
    /// let wildcard = RadixHost::new("*.example.com", runtime);
    /// assert_eq!(wildcard.radix_key, "com.example");
    /// assert!(wildcard.is_wildcard);
    /// assert_eq!(wildcard.wildcard_count, 1);
    /// ```
    pub fn new(host: &str, runtime: Arc<T>) -> Self {
        let original = host.to_string();
        let host_lower = host.to_lowercase();

        let is_wildcard = host_lower.starts_with('*');
        let mut wildcard_count = 0;

        // Extract the fixed part (without wildcards)
        let fixed_part = if is_wildcard {
            // Count leading wildcards: *.*.example.com -> 2 wildcards
            let parts: Vec<&str> = host_lower.split('.').collect();
            for part in &parts {
                if *part == "*" {
                    wildcard_count += 1;
                } else {
                    break;
                }
            }
            parts[wildcard_count..].join(".")
        } else {
            host_lower.clone()
        };

        let radix_key = Self::reverse_hostname(&fixed_part);

        RadixHost {
            original,
            radix_key,
            is_wildcard,
            wildcard_count,
            runtime,
        }
    }

    /// Reverse a hostname by splitting on `.` and joining the parts in reverse order.
    ///
    /// - `”example.com”` -> `”com.example”`
    /// - `”api.example.com”` -> `”com.example.api”`
    ///
    /// The caller is responsible for stripping wildcard segments before passing to
    /// this function. `RadixHost::new` does this automatically.
    pub fn reverse_hostname(host: &str) -> String {
        if host.is_empty() {
            return String::new();
        }

        host.split('.').rev().collect::<Vec<_>>().join(".")
    }

    /// Check if a pre-normalized (lowercased) hostname matches this pattern.
    ///
    /// The caller MUST pass a hostname that is already lowercased.
    /// Normalization happens once in `build_request_metadata()`.
    ///
    /// # Arguments
    /// * `request_host` - Pre-normalized lowercase hostname
    ///
    /// # Returns
    /// `true` if the hostname matches this pattern
    pub fn matches(&self, request_host: &str) -> bool {
        let reversed_request = Self::reverse_hostname(request_host);
        self.matches_reversed(&reversed_request)
    }

    /// Like `matches()`, but accepts a hostname already reversed by `reverse_hostname()`.
    /// Use this when the caller has already reversed the hostname to avoid a second allocation.
    pub fn matches_reversed(&self, reversed_request: &str) -> bool {
        if self.is_wildcard {
            self.wildcard_matches(reversed_request)
        } else {
            reversed_request == self.radix_key
        }
    }

    /// Check if a reversed hostname matches a wildcard pattern
    fn wildcard_matches(&self, reversed_request: &str) -> bool {
        // Check if the reversed request starts with the radix_key
        if !reversed_request.starts_with(&self.radix_key) {
            return false;
        }

        // Wildcard requires at least one segment beyond the fixed prefix.
        if reversed_request.len() == self.radix_key.len() {
            return false; // No room for wildcard segments
        }

        // After the radix_key, there should be a dot separator
        let remaining = &reversed_request[self.radix_key.len()..];
        if !remaining.starts_with('.') {
            return false;
        }

        // Count the number of segments in the remaining part
        let remaining_without_dot = &remaining[1..]; // Skip the leading dot
        if remaining_without_dot.is_empty() {
            return false;
        }

        // Count segments (number of parts separated by dots)
        let segment_count = remaining_without_dot.split('.').count();

        // Per Gateway API spec, wildcards match any number of labels (suffix match)
        segment_count >= self.wildcard_count
    }
}
