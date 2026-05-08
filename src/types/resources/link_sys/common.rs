//! LinkSys common types.
//!
//! `SecretObjectReference` has been promoted to
//! `crate::types::resources::common::SecretObjectReference`. This module only
//! re-exports it to keep `use` paths within the LinkSys module stable.

pub use crate::types::resources::common::SecretObjectReference;
