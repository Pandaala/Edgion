pub mod base_conf_loader;
pub mod conf_center;
pub mod conf_mgr_trait;
pub mod resource_check;
pub mod schema_validator;

pub use conf_mgr_trait::{ConfMgrError, EdgionConfMgr};
pub use schema_validator::SchemaValidator;

// ConfCenter exports
pub use conf_center::{
    load_all_resources, ConfCenter, ConfCenterConfig, ConfEntry, ConfWriter, ConfWriterError, ControllerExitReason,
    FileSystemStatusStore, FileSystemWriter, FileWatcher, KubernetesController, KubernetesStatusStore,
    KubernetesWriter, MetadataFilterConfig, NamespaceWatchMode, RelinkReason, StatusStore, StatusStoreError,
};

// Backward compatibility aliases (direct re-exports)
pub use conf_center::ConfWriterError as ConfStoreError;
