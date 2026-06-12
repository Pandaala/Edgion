//! Configuration Center top-level config.
//!
//! The schema definition lives in `crate::config::conf_center`. Re-exported here
//! to keep existing controller call sites stable.

pub use crate::config::conf_center::config::ConfCenterConfig;
