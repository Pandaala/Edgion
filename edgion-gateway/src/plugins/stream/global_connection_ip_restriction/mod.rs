//! `GlobalConnectionIpRestriction` stream-plugin facade.
//!
//! The plugin evaluates inline, compiled named profiles plus an optional
//! `EdgionConfigData` selector. It runs at listener or route-owned stream
//! lifecycle points and always evaluates `DirectPeerIp`.

pub mod plugin;

pub use plugin::GlobalConnectionIpRestrictionPlugin;
