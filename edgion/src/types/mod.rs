pub mod err;
pub mod global_def;
pub mod resource_kind;
pub mod resources;
pub mod schema;
pub mod versionable;

pub use self::err::{EdError, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
pub use self::global_def::*;
pub use self::resource_kind::ResourceKind;
pub use self::resources::*;
pub use self::schema::*;
pub use self::versionable::{ResourceMeta, Versionable};

pub mod prelude_resources {
    // Re-export all resource types
    pub use super::resources::*;
    
    // Re-export ResourceKind enum
    pub use super::resource_kind::ResourceKind;
    
    // Re-export ResourceMeta trait (and Versionable alias for backward compatibility)
    pub use super::versionable::{ResourceMeta, Versionable};
}
