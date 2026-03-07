mod config_client;
mod grpc_client;

pub use config_client::{ConfigClient, ListDataSimple};
pub use grpc_client::ConfigSyncClient;
