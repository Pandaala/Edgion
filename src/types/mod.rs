pub mod ctx;
pub mod edgion_status;
pub mod err;
pub mod filters;
pub mod gateway_base_conf;
pub mod global_def;
pub mod link_sys;
pub mod observe;
pub mod resource_kind;
pub mod resource_meta_traits;
pub mod resource_registry;
pub mod resources;
pub mod schema;
pub mod work_dir;

pub use self::ctx::{BackendContext, BackendTlsInfo, EdgionHttpContext, MatchInfo, RequestInfo, UpstreamInfo};
pub use self::edgion_status::EdgionStatus;
pub use self::err::{EdError, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
pub use self::gateway_base_conf::GatewayBaseConf;
pub use self::global_def::*;
pub use self::link_sys::{LocalFileWriterCfg, LocalFileWriterConfig, StringOutput};
pub use self::observe::{LogConfig, LogType};
pub use self::resource_kind::ResourceKind;
pub use self::resource_meta_traits::ResourceMeta;
pub use self::resource_registry::{
    all_resource_type_names, base_conf_resource_names, get_resource_metadata, ResourceTypeMetadata, RESOURCE_TYPES,
};
pub use self::resources::*;
pub use self::schema::*;
pub use self::work_dir::{init_work_dir, work_dir, WorkDir};

// Re-export from conf_mgr for backward compatibility
pub use crate::core::conf_mgr::{
    ConfEntry as ResourceEntry, ConfMgrError as ResourceMgrError, ConfStore as ResourceStore,
    ConfStoreError as ResourceStoreError, EdgionConfMgr as EdgionResourceMgr,
};

pub mod prelude_resources {
    // Re-export all resource types
    pub use super::resources::*;

    // Re-export ResourceKind enum
    pub use super::resource_kind::ResourceKind;

    // Re-export ResourceMeta trait
    pub use super::resource_meta_traits::ResourceMeta;
}
