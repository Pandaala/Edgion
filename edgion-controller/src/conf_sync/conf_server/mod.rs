//! conf_server: gRPC list/watch service for configuration synchronization.
//!
//! `ServerCache<T>` instances are managed by `ResourceProcessor` in `conf_mgr`.
//!
//! Key components:
//! - `WatchObj` trait: Object-safe interface for list/watch operations
//! - `ConfigSyncServer`: Holds `HashMap<kind, Arc<dyn WatchObj>>`; registered by processors
//! - `ConfigSyncGrpcServer`: gRPC service implementation

mod client_registry;
mod config_sync_server;
mod grpc_server;
mod traits;

pub use client_registry::ClientRegistry;
pub use config_sync_server::ConfigSyncServer;
pub use grpc_server::{ConfigSyncGrpcServer, ConfigSyncServerProvider};
pub use traits::{WatchObj, WatchResponseSimple};
