//! Configuration Center top-level config.
//!
//! The schema definition now lives in `common::config::conf_center` so that
//! shared config loading can reference it without a `common -> controller`
//! dependency. Re-exported here to keep existing controller call sites stable.

pub use crate::core::common::config::conf_center::config::ConfCenterConfig;
