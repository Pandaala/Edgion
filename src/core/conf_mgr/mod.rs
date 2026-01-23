pub mod conf_center;
pub mod conf_mgr_trait;
pub mod schema_validator;

pub use conf_mgr_trait::{ConfMgrError, EdgionConfMgr};
pub use schema_validator::SchemaValidator;

// ConfCenter exports
pub use conf_center::{
    ConfCenter, ConfCenterConfig, ConfEntry, ConfWriter, ConfWriterError, ControllerExitReason,
    FileSystemStatusStore, FileSystemSyncController, FileSystemWriter, KubernetesController, KubernetesStatusStore,
    KubernetesWriter, MetadataFilterConfig, NamespaceWatchMode, RelinkReason, StatusStore, StatusStoreError,
};

// Backward compatibility aliases (direct re-exports)
pub use conf_center::ConfWriterError as ConfStoreError;
