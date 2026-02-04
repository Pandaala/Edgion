//! New simplified conf_server module
//!
//! This module provides a simplified ConfigSyncServer that only handles gRPC list/watch.
//! The ServerCache<T> instances are managed by ResourceProcessor in conf_mgr.
//!
//! Key components:
//! - `WatchObj` trait: Object-safe interface for list/watch operations
//! - `ConfigSyncServer`: Simplified server that holds HashMap<kind, Arc<dyn WatchObj>>
//! - `ConfigSyncGrpcServer`: gRPC service implementation

mod config_sync_server;
mod grpc_server;
mod traits;

pub use config_sync_server::ConfigSyncServer;
pub use grpc_server::{ConfigSyncGrpcServer, ConfigSyncServerProvider};
pub use traits::{WatchObj, WatchResponseSimple};
