//! Error types for IP radix matching

use std::fmt;

/// Errors that can occur during IP radix tree operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpRadixError {
    /// Invalid CIDR notation
    InvalidCidr {
        /// The invalid CIDR string
        input: String,
        /// Reason for the error
        reason: String,
    },

    /// Prefix length exceeds the maximum allowed for the IP version
    PrefixTooLong {
        /// The actual prefix length
        prefix_len: u8,
        /// The maximum allowed prefix length
        max: u8,
    },

    /// Too many nodes in the tree (exceeds u32::MAX)
    TooManyNodes {
        /// The actual node count
        count: usize,
        /// The maximum allowed count
        max: usize,
    },

    /// Empty tree cannot be frozen
    EmptyTree,

    /// IP address parsing error
    InvalidIpAddress {
        /// The invalid IP string
        input: String,
        /// Error message from parsing
        error: String,
    },

    /// Same normalised CIDR appears in two distinct group labels in the
    /// same matcher. Tightens v3's silent-overwrite behaviour.
    DuplicateCidr {
        /// The normalised CIDR string that was inserted twice.
        cidr: String,
        /// Group name of the first insertion (winner under old semantics).
        existing_label: String,
        /// Group name of the second insertion that triggered the error.
        new_label: String,
    },

    /// Builder reached the maximum number of distinct group labels
    /// (`u16` capacity minus reserved IDs).
    TooManyGroups {
        /// Maximum distinct groups allowed (= 65534).
        max_labels: u16,
    },
}

impl fmt::Display for IpRadixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpRadixError::InvalidCidr { input, reason } => {
                write!(f, "invalid CIDR '{}': {}", input, reason)
            }
            IpRadixError::PrefixTooLong { prefix_len, max } => {
                write!(f, "prefix length {} exceeds maximum {}", prefix_len, max)
            }
            IpRadixError::TooManyNodes { count, max } => {
                write!(f, "tree has {} nodes, exceeds maximum {}", count, max)
            }
            IpRadixError::EmptyTree => {
                write!(f, "cannot freeze an empty tree")
            }
            IpRadixError::InvalidIpAddress { input, error } => {
                write!(f, "invalid IP address '{}': {}", input, error)
            }
            IpRadixError::DuplicateCidr {
                cidr,
                existing_label,
                new_label,
            } => {
                write!(
                    f,
                    "duplicate CIDR '{}' across groups '{}' and '{}'",
                    cidr, existing_label, new_label
                )
            }
            IpRadixError::TooManyGroups { max_labels } => {
                write!(f, "too many distinct groups in one matcher (max {})", max_labels)
            }
        }
    }
}

impl std::error::Error for IpRadixError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_cidr_display() {
        let e = IpRadixError::DuplicateCidr {
            cidr: "10.0.0.0/24".to_string(),
            existing_label: "office".to_string(),
            new_label: "corp".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("10.0.0.0/24"));
        assert!(s.contains("office"));
        assert!(s.contains("corp"));
    }

    #[test]
    fn too_many_groups_display() {
        let e = IpRadixError::TooManyGroups { max_labels: 65534 };
        let s = e.to_string();
        assert!(s.contains("65534"));
    }

    #[test]
    fn existing_variants_still_render() {
        // Confirms no existing arm was lost during the edit.
        let e = IpRadixError::EmptyTree;
        assert_eq!(e.to_string(), "cannot freeze an empty tree");
    }
}
