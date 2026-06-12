//! FileSystem backend configuration.
//!
//! The schema definition now lives in `common::config::conf_center::file_system`.
//! Re-exported here to keep existing controller call sites stable.

pub use crate::core::common::config::conf_center::file_system::FileSystemConfig;
