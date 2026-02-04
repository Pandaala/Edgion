//! Centralized constants module
//!
//! This module provides a single source of truth for all string constants
//! used throughout the codebase, including:
//! - Kubernetes labels and annotations
//! - HTTP headers
//! - Secret data keys
//! - Application identity and component names
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::types::constants::labels::k8s::SERVICE_NAME;
//! use crate::types::constants::secret_keys::tls::{CERT, KEY};
//! use crate::types::constants::headers::proxy::X_FORWARDED_FOR;
//! use crate::types::constants::app::{CONTROLLER_NAME, GATEWAY_NAME};
//! ```

pub mod annotations;
pub mod app;
pub mod headers;
pub mod labels;
pub mod secret_keys;

// Re-export commonly used constants at module level
pub use app::*;
