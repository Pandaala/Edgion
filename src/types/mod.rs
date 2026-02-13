pub mod common;
pub mod constants;
pub mod ctx;
pub mod edgion_status;
pub mod err;
pub mod filters;
pub mod gateway_base_conf;
pub mod observe;
pub mod output;
pub mod resources;
pub mod schema;
pub mod work_dir;

// Re-export common types
pub use self::common::{KeyGet, KeySet};

// Backward compatibility: global_def re-exports from constants::app
pub mod global_def {
    pub use super::constants::app::*;
}

// Resource system core module (consolidated)
#[macro_use]
pub mod resource;

// Re-export from output module (previously link_sys)
pub use self::output::{LocalFileWriterCfg, LocalFileWriterConfig, RotationConfig, RotationStrategy, StringOutput};

// Backward compatibility: link_sys module alias for output types
pub mod link_sys {
    pub use super::output::*;
}

// Re-export from resource module
pub use self::resource::ResourceKind;
pub use self::resource::ResourceMeta;
pub use self::resource::{
    all_resource_type_names, base_conf_resource_names, get_resource_metadata, ResourceTypeMetadata,
    DEFAULT_NO_SYNC_KINDS, RESOURCE_TYPES,
};

// Re-export from other modules
pub use self::constants::app::*;
pub use self::ctx::{
    BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, MatchInfo, RequestInfo, TlsConnId,
    UpstreamInfo,
};
pub use self::edgion_status::EdgionStatus;
pub use self::err::{
    EdError, WATCH_ERR_EVENTS_LOST, WATCH_ERR_NOT_READY, WATCH_ERR_SERVER_ID_MISMATCH, WATCH_ERR_SERVER_RELOAD,
    WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED,
};
pub use self::gateway_base_conf::GatewayBaseConf;
pub use self::observe::{LogConfig, LogType};
pub use self::resources::*;
pub use self::schema::*;
pub use self::work_dir::{init_work_dir, work_dir, WorkDir};

// Re-export from conf_mgr for backward compatibility
pub use crate::core::conf_mgr::{
    CenterApi as ResourceStore, ConfEntry as ResourceEntry, ConfStoreError as ResourceStoreError,
};

// Backward compatibility re-exports (old paths)
pub mod resource_kind {
    pub use super::resource::kind::*;
}
pub mod resource_defs {
    pub use super::resource::defs::*;
}
// resource_macros is #[macro_use], no re-export needed
pub mod resource_registry {
    pub use super::resource::registry::*;
}
pub mod resource_meta_traits {
    pub use super::resource::meta::*;
}

pub mod prelude_resources {
    // Re-export all resource types
    pub use super::resources::*;

    // Re-export ResourceKind enum
    pub use super::resource::kind::ResourceKind;

    // Re-export ResourceMeta trait
    pub use super::resource::meta::ResourceMeta;
}
