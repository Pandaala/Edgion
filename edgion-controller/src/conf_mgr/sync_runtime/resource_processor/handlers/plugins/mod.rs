//! EdgionPlugins helper modules.
//!
//! Houses per-plugin-type credential resolution split out of `edgion_plugins.rs`
//! to keep that handler focused on dispatch and status management.

pub(super) mod condition_tree;
pub(super) mod request_access_policy_data;
pub(super) mod resolve_credential;

pub(super) mod grpc_descriptor;
