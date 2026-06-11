/// Check if a string is a valid domain name
/// Valid domain: alphanumeric, hyphens, dots, but not starting/ending with hyphen or dot
/// Must contain at least one dot
pub fn is_valid_domain(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let bytes = s.as_bytes();
    let len = bytes.len();

    let mut has_dot = false;
    let mut label_start = 0;
    let mut i = 0;

    while i <= len {
        // Check if we're at a dot or end of string
        if i == len || bytes[i] == b'.' {
            // Check if ends with dot
            if i == len && i > 0 && bytes[i - 1] == b'.' {
                return false;
            }

            // Mark that we found a dot
            if i < len && bytes[i] == b'.' {
                has_dot = true;
            }

            // Validate the label between label_start and i
            let label_len = i - label_start;

            if label_len == 0 {
                // Empty label (e.g., ".." or starts with ".")
                return false;
            }

            // Check first character of label (can't be hyphen)
            if bytes[label_start] == b'-' {
                return false;
            }

            // Check last character of label (can't be hyphen)
            if bytes[i - 1] == b'-' {
                return false;
            }

            // Check all characters in label
            for &ch in &bytes[label_start..i] {
                if !ch.is_ascii_alphanumeric() && ch != b'-' {
                    return false;
                }
            }

            // Move to next label
            label_start = i + 1;
        }

        i += 1;
    }

    // Must have at least one dot
    has_dot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_domains() {
        assert!(is_valid_domain("example.com"));
        assert!(is_valid_domain("my-site.com"));
        assert!(is_valid_domain("api.example.com"));
        assert!(is_valid_domain("api2.example.com"));
        assert!(is_valid_domain("my-api-v2.com"));
        assert!(is_valid_domain("123.com"));
        assert!(is_valid_domain("a.b.c.d.e.com"));
        assert!(is_valid_domain("a.b"));
        assert!(is_valid_domain("test-123.example-456.com"));
    }

    #[test]
    fn test_invalid_domains() {
        // Empty
        assert!(!is_valid_domain(""));

        // No dot (single label)
        assert!(!is_valid_domain("localhost"));
        assert!(!is_valid_domain("example"));

        // Empty label
        assert!(!is_valid_domain(".com"));
        assert!(!is_valid_domain("example..com"));
        assert!(!is_valid_domain("example.com."));

        // Hyphen at start/end
        assert!(!is_valid_domain("-example.com"));
        assert!(!is_valid_domain("example-.com"));
        assert!(!is_valid_domain("example.-com"));

        // Invalid characters
        assert!(!is_valid_domain("exam_ple.com"));
        assert!(!is_valid_domain("exam ple.com"));
        assert!(!is_valid_domain("example.com!"));
        assert!(!is_valid_domain("*.example.com"));
        assert!(!is_valid_domain("example@.com"));
        assert!(!is_valid_domain(".example.com"));
    }
}
