//! LinkSys common types.
//!
//! `SecretObjectReference` 已提升到 `crate::types::resources::common::SecretObjectReference`，
//! 此处仅 re-export 保持 LinkSys 模块内的 use 路径稳定。

pub use crate::types::resources::common::SecretObjectReference;
