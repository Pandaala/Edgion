pub mod common;
pub mod constants;
pub mod credentials;
pub mod ctx;
pub mod edgion_status;
pub mod err;
pub mod filters;
pub mod gateway_base_conf;
pub mod listener_key;
pub mod observe;
pub mod output;
pub mod resources;
pub mod schema;
pub mod sensitive;
pub mod uri_utils;
pub mod work_dir;

// Re-export common types
pub use self::common::{KeyGet, KeySet};
pub use self::listener_key::ListenerKey;

// Backward compatibility: global_def re-exports from constants::app
pub mod global_def {
    pub use super::constants::app::*;
}

// Resource system core module (consolidated)
#[macro_use]
pub mod resource;

pub use self::output::{LocalFileWriterCfg, LocalFileWriterConfig, RotationConfig, RotationStrategy, StringOutput};

// Re-export from resource module
pub use self::resource::ResourceKind;
pub use self::resource::ResourceMeta;
pub use self::resource::{
    all_resource_type_names, base_conf_resource_names, default_capacity_for_kind, get_resource_metadata,
    ResourceTypeMetadata, DEFAULT_NO_SYNC_KINDS, RESOURCE_TYPES,
};

// Re-export from other modules
pub use self::constants::app::*;
pub use self::ctx::{
    BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, ExternalJumpPreset, InternalJumpPreset,
    MatchInfo, MatchedInfo, MirrorConfig, MirrorState, PendingHeaderOp, RequestInfo, Rsc, SessionPersistenceState,
    SetJumpOutcome, TlsConnId, TlsConnMeta, UpstreamInfo, UpstreamJumpTarget,
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
pub use crate::core::controller::conf_mgr::{
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
