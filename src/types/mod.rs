pub mod err;
pub mod edgion_status;
pub mod global_def;
pub mod resource_kind;
pub mod resource_registry;
pub mod resources;
pub mod schema;
pub mod resource_meta_traits;
pub mod gateway_base_conf;
pub mod ctx;
pub mod link_sys;
pub mod filters;

pub use self::err::{EdError, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
pub use self::edgion_status::EdgionStatus;
pub use self::global_def::*;
pub use self::resource_kind::ResourceKind;
pub use self::resource_registry::{ResourceTypeMetadata, RESOURCE_TYPES, all_resource_type_names, base_conf_resource_names, get_resource_metadata};
pub use self::resources::*;
pub use self::schema::*;
pub use self::resource_meta_traits::ResourceMeta;
pub use self::gateway_base_conf::GatewayBaseConf;
pub use self::ctx::{BackendContext, BackendTlsInfo, MatchInfo, RequestInfo, UpstreamInfo, EdgionHttpContext};
pub use self::link_sys::{LocalFileWriterConfig, LocalFileWriterCfg, StringOutput};

// Re-export from conf_mgr for backward compatibility
pub use crate::core::conf_mgr::{
    ConfStore as ResourceStore,
    ConfEntry as ResourceEntry,
    ConfStoreError as ResourceStoreError,
    EdgionConfMgr as EdgionResourceMgr,
    ConfMgrError as ResourceMgrError,
};

pub mod prelude_resources {
    // Re-export all resource types
    pub use super::resources::*;
    
    // Re-export ResourceKind enum
    pub use super::resource_kind::ResourceKind;
    
    // Re-export ResourceMeta trait
    pub use super::resource_meta_traits::ResourceMeta;
}
