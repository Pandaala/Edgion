pub mod cache;
pub mod registry;
pub mod traits;

pub use cache::{CenterWatchCache, WatchEventSimple};
pub use registry::CenterWatchCacheRegistry;
pub use traits::CenterConfHandler;

use edgion_resources::resources::plugin_metadata::PluginMetaData;

/// Top-level container for all per-kind watch registries.
/// Mirrors Gateway's ConfigClient — add a field for each new resource kind.
pub struct CenterSyncClient {
    pub plugin_metadata: CenterWatchCacheRegistry<PluginMetaData>,
    // Future: pub http_routes: CenterWatchCacheRegistry<HTTPRoute>,
}
