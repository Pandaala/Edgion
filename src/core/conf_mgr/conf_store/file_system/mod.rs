mod store_impl;

use std::path::PathBuf;
use std::sync::Arc;

/// File system based resource storage
/// Stores resources as YAML files with naming convention: Kind_namespace_name.yaml
pub struct FileSystemStore {
    root: PathBuf,
}

impl FileSystemStore {
    /// Create a new file system store with absolute root path
    pub fn new<P: Into<PathBuf>>(root: P) -> Arc<Self> {
        let root_path = root.into();
        let root_abs = if root_path.is_absolute() {
            root_path
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&root_path))
                .unwrap_or(root_path)
        };
        
        tracing::info!(
            component = "file_system_store",
            event = "init",
            root = ?root_abs,
            "Initialized FileSystemStore"
        );
        
        Arc::new(Self { root: root_abs })
    }
    
    pub fn root(&self) -> &PathBuf {
        &self.root
    }
}

