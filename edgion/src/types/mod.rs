pub mod err;
pub mod err_code;
pub mod global_def;
pub mod resource_kind;
pub mod resources;
pub mod schema;
pub mod resource_meta_traits;
pub mod gateway_base_conf;

pub use self::err::{EdError, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
pub use self::err_code::EdgionErrStatus;
pub use self::global_def::*;
pub use self::resource_kind::ResourceKind;
pub use self::resources::*;
pub use self::schema::*;
pub use self::resource_meta_traits::ResourceMeta;
pub use self::gateway_base_conf::GatewayBaseConf;

pub mod prelude_resources {
    // Re-export all resource types
    pub use super::resources::*;
    
    // Re-export ResourceKind enum
    pub use super::resource_kind::ResourceKind;
    
    // Re-export ResourceMeta trait
    pub use super::resource_meta_traits::ResourceMeta;
}
