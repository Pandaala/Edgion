//! LinkSys common types.
//!
//! `SecretReference` 已提升到 `crate::types::resources::common::SecretReference`，
//! 此处仅 re-export 保持 LinkSys 模块内的 use 路径稳定。

pub use crate::types::resources::common::SecretReference;
