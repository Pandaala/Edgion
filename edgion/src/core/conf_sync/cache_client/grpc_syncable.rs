use crate::types::ResourceKind;
use serde::de::DeserializeOwned;

/// Trait for resources that can be synced via gRPC
pub trait GrpcSyncable: DeserializeOwned + Send + Sync + 'static {
    /// Get the ResourceKind for this type
    fn resource_kind() -> ResourceKind;

    /// Get a human-readable name for logging
    fn kind_name() -> &'static str;
}
