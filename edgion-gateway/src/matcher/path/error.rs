//! Error types for the radix router

use std::fmt;

/// Errors that can occur during router operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathRadixError {
    /// Path is empty
    EmptyPath,

    /// Path length exceeds the maximum allowed (u16::MAX = 65535)
    PathTooLong {
        /// The actual path length
        length: usize,
        /// The maximum allowed length
        max: usize,
    },

    /// Too many values for a single path (exceeds u8::MAX = 255)
    TooManyValues {
        /// The path that has too many values
        path: String,
        /// The current count
        count: usize,
        /// The maximum allowed count
        max: usize,
    },

    /// Too many nodes in the tree (exceeds u32::MAX)
    TooManyNodes {
        /// The actual node count
        count: usize,
        /// The maximum allowed count
        max: usize,
    },

    /// Too many children for a single node (exceeds u16::MAX = 65535)
    TooManyChildren {
        /// The actual children count
        count: usize,
        /// The maximum allowed count
        max: usize,
    },

    /// String pool size exceeds u32::MAX
    StringPoolTooLarge {
        /// The actual size
        size: usize,
        /// The maximum allowed size
        max: usize,
    },

    /// Values pool size exceeds u32::MAX
    ValuesPoolTooLarge {
        /// The actual size
        size: usize,
        /// The maximum allowed size
        max: usize,
    },
}

impl fmt::Display for PathRadixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathRadixError::EmptyPath => {
                write!(f, "path cannot be empty")
            }
            PathRadixError::PathTooLong { length, max } => {
                write!(f, "path length {} exceeds maximum {}", length, max)
            }
            PathRadixError::TooManyValues { path, count, max } => {
                write!(f, "path '{}' has {} values, exceeds maximum {}", path, count, max)
            }
            PathRadixError::TooManyNodes { count, max } => {
                write!(f, "tree has {} nodes, exceeds maximum {}", count, max)
            }
            PathRadixError::TooManyChildren { count, max } => {
                write!(f, "node has {} children, exceeds maximum {}", count, max)
            }
            PathRadixError::StringPoolTooLarge { size, max } => {
                write!(f, "string pool size {} exceeds maximum {}", size, max)
            }
            PathRadixError::ValuesPoolTooLarge { size, max } => {
                write!(f, "values pool size {} exceeds maximum {}", size, max)
            }
        }
    }
}

impl std::error::Error for PathRadixError {}
