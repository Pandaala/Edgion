mod api;
pub mod base_conf_loader;
pub mod conf_center;
pub mod conf_mgr_trait;
pub mod resource_check;
pub mod schema_validator;

pub use api::ResourceMgrAPI;
pub use base_conf_loader::load_base_conf_from_store;
pub use conf_mgr_trait::{ConfMgrError, EdgionConfMgr};
pub use schema_validator::SchemaValidator;

// ConfCenter exports
pub use conf_center::{
    ConfCenter, ConfCenterConfig, ConfEntry, ConfWriter, ConfWriterError,
    FileSystemStatusStore, FileSystemWriter, KubernetesController, KubernetesStatusStore,
    KubernetesStore, KubernetesWriter, StatusStore, StatusStoreError, load_all_resources,
};

// Backward compatibility aliases (direct re-exports)
pub use conf_center::ConfWriterError as ConfStoreError;
