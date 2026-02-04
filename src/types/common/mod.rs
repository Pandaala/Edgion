//! Common types shared across the codebase
//!
//! This module contains unified type definitions that are used by multiple
//! plugins and components.
//!
//! ## Key Accessor Types
//!
//! - `KeyGet`: Read values from request context (headers, cookies, query, etc.)
//! - `KeySet`: Write values to request/response context
//!
//! These types are used with `PluginSession::key_get()` and `PluginSession::key_set()`.

mod key_accessor;

pub use key_accessor::{KeyGet, KeySet};
