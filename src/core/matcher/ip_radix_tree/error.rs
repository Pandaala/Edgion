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
}

impl fmt::Display for IpRadixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpRadixError::InvalidCidr { input, reason } => {
                write!(f, "invalid CIDR '{}': {}", input, reason)
            }
            IpRadixError::PrefixTooLong { prefix_len, max } => {
                write!(
                    f,
                    "prefix length {} exceeds maximum {}",
                    prefix_len, max
                )
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
        }
    }
}

impl std::error::Error for IpRadixError {}