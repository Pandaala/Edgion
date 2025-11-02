use std::sync::Arc;

/// RadixHost represents a hostname pattern for radix tree matching
/// Hostnames are reversed for longest prefix matching
/// e.g., "api.example.com" becomes "com.example.api"
///      "*.example.com" becomes "com.example" (radix_key for prefix matching)
#[derive(Clone)]
pub struct RadixHost<T> {
    pub original: String,
    pub radix_key: String,
    pub is_wildcard: bool,
    pub wildcard_count: usize,
    pub runtime: Arc<T>,
}

impl<T> RadixHost<T> {
    /// Create a new RadixHost from a hostname pattern
    ///
    /// # Arguments
    /// * `host` - The hostname pattern (e.g., "example.com" or "*.example.com")
    /// * `runtime` - The runtime associated with this host
    ///
    /// # Examples
    /// ```
    /// let host = RadixHost::new("api.example.com", runtime);
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

        // Check if it's a wildcard pattern and count wildcards
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
            // Join the non-wildcard parts
            parts[wildcard_count..].join(".")
        } else {
            host_lower.clone()
        };

        // Reverse the fixed part for radix tree matching
        let radix_key = Self::reverse_hostname(&fixed_part);

        RadixHost {
            original,
            radix_key,
            is_wildcard,
            wildcard_count,
            runtime,
        }
    }

    /// Reverse a hostname for radix tree matching
    /// Handles wildcard patterns by keeping wildcards at the end after reversal
    ///
    /// Examples:
    /// - "example.com" -> "moc.elpmaxe"
    /// - "api.example.com" -> "moc.elpmaxe.ipa"
    /// - "*.example.com" -> "moc.elpmaxe.*"
    /// - "*.*.example.com" -> "moc.elpmaxe.*.*"
    pub fn reverse_hostname(host: &str) -> String {
        if host.is_empty() {
            return String::new();
        }

        // Split by dots, reverse the parts, then join
        let parts: Vec<&str> = host.split('.').collect();
        let reversed_parts: Vec<&str> = parts.into_iter().rev().collect();
        reversed_parts.join(".")
    }

    /// Check if a request hostname matches this pattern
    ///
    /// # Arguments
    /// * `request_host` - The hostname from the request (will be lowercased)
    ///
    /// # Returns
    /// `true` if the hostname matches this pattern
    pub fn matches(&self, request_host: &str) -> bool {
        let request_lower = request_host.to_lowercase();
        let reversed_request = Self::reverse_hostname(&request_lower);

        if self.is_wildcard {
            // For wildcard patterns, check if reversed request starts with the radix_key
            // and has the correct number of additional segments
            self.wildcard_matches(&reversed_request)
        } else {
            // Exact match
            reversed_request == self.radix_key
        }
    }

    /// Check if a reversed hostname matches a wildcard pattern
    fn wildcard_matches(&self, reversed_request: &str) -> bool {
        // Check if the reversed request starts with the radix_key
        if !reversed_request.starts_with(&self.radix_key) {
            return false;
        }

        // If radix_key is the entire request, we need wildcards to match something
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

        // Must match exactly the wildcard count
        segment_count == self.wildcard_count
    }
}
