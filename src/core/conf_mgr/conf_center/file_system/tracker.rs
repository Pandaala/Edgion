//! File Resource Tracker
//!
//! Maintains bidirectional mapping between file paths and resources.
//! Used by FileSystemSyncController to:
//! - Track which file contains which resource
//! - Detect file deletions and map them to resource deletions
//! - Support content hash for change detection

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Tracks the relationship between files and resources
///
/// Maintains two mappings:
/// - `path_to_resource`: file path -> (kind, key, content_hash)
/// - `resource_to_path`: (kind, key) -> file path
#[derive(Debug, Default)]
pub struct FileResourceTracker {
    /// Forward mapping: path -> (kind, key, content_hash)
    path_to_resource: HashMap<PathBuf, (String, String, u64)>,
    /// Reverse mapping: (kind, key) -> path
    resource_to_path: HashMap<(String, String), PathBuf>,
}

impl FileResourceTracker {
    /// Create a new empty tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Track a file-resource relationship
    ///
    /// If the same path was previously tracked with a different resource,
    /// the old resource mapping is removed first.
    pub fn track(&mut self, path: PathBuf, kind: &str, key: &str, hash: u64) {
        // Remove old mapping if path was previously tracked
        if let Some((old_kind, old_key, _)) = self.path_to_resource.remove(&path) {
            self.resource_to_path.remove(&(old_kind, old_key));
        }

        // Also remove if the same resource was in a different file
        let resource_key = (kind.to_string(), key.to_string());
        if let Some(old_path) = self.resource_to_path.remove(&resource_key) {
            if old_path != path {
                self.path_to_resource.remove(&old_path);
            }
        }

        // Add new mappings
        self.path_to_resource
            .insert(path.clone(), (kind.to_string(), key.to_string(), hash));
        self.resource_to_path.insert(resource_key, path);
    }

    /// Remove tracking for a file path
    ///
    /// Returns the (kind, key) if the path was tracked, None otherwise.
    pub fn untrack(&mut self, path: &Path) -> Option<(String, String)> {
        if let Some((kind, key, _)) = self.path_to_resource.remove(path) {
            self.resource_to_path.remove(&(kind.clone(), key.clone()));
            Some((kind, key))
        } else {
            None
        }
    }

    /// Get resource info for a file path
    ///
    /// Returns (kind, key, content_hash) if tracked.
    pub fn get_by_path(&self, path: &Path) -> Option<(&str, &str, u64)> {
        self.path_to_resource
            .get(path)
            .map(|(kind, key, hash)| (kind.as_str(), key.as_str(), *hash))
    }

    /// Get file path for a resource
    pub fn get_path_by_key(&self, kind: &str, key: &str) -> Option<&PathBuf> {
        self.resource_to_path.get(&(kind.to_string(), key.to_string()))
    }

    /// Check if a resource is tracked
    pub fn has_key(&self, kind: &str, key: &str) -> bool {
        self.resource_to_path
            .contains_key(&(kind.to_string(), key.to_string()))
    }

    /// Check if a file path is tracked
    pub fn has_path(&self, path: &Path) -> bool {
        self.path_to_resource.contains_key(path)
    }

    /// Get the content hash for a file path
    pub fn get_hash(&self, path: &Path) -> Option<u64> {
        self.path_to_resource.get(path).map(|(_, _, hash)| *hash)
    }

    /// Update only the content hash for an existing path
    ///
    /// Returns true if the path was tracked and hash was updated.
    pub fn update_hash(&mut self, path: &Path, new_hash: u64) -> bool {
        if let Some((_, _, hash)) = self.path_to_resource.get_mut(path) {
            *hash = new_hash;
            true
        } else {
            false
        }
    }

    /// Get the number of tracked resources
    pub fn len(&self) -> usize {
        self.path_to_resource.len()
    }

    /// Check if the tracker is empty
    pub fn is_empty(&self) -> bool {
        self.path_to_resource.is_empty()
    }

    /// Clear all tracked resources
    pub fn clear(&mut self) {
        self.path_to_resource.clear();
        self.resource_to_path.clear();
    }

    /// Iterate over all tracked (path, kind, key)
    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &str, &str)> {
        self.path_to_resource
            .iter()
            .map(|(path, (kind, key, _))| (path, kind.as_str(), key.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_and_get() {
        let mut tracker = FileResourceTracker::new();
        let path = PathBuf::from("/conf/gateway.yaml");

        tracker.track(path.clone(), "Gateway", "default/my-gateway", 12345);

        // Check forward lookup
        let (kind, key, hash) = tracker.get_by_path(&path).unwrap();
        assert_eq!(kind, "Gateway");
        assert_eq!(key, "default/my-gateway");
        assert_eq!(hash, 12345);

        // Check reverse lookup
        let found_path = tracker.get_path_by_key("Gateway", "default/my-gateway").unwrap();
        assert_eq!(found_path, &path);

        // Check has methods
        assert!(tracker.has_path(&path));
        assert!(tracker.has_key("Gateway", "default/my-gateway"));
    }

    #[test]
    fn test_untrack() {
        let mut tracker = FileResourceTracker::new();
        let path = PathBuf::from("/conf/route.yaml");

        tracker.track(path.clone(), "HTTPRoute", "default/my-route", 67890);
        assert!(tracker.has_path(&path));

        let (kind, key) = tracker.untrack(&path).unwrap();
        assert_eq!(kind, "HTTPRoute");
        assert_eq!(key, "default/my-route");

        // Both mappings should be removed
        assert!(!tracker.has_path(&path));
        assert!(!tracker.has_key("HTTPRoute", "default/my-route"));
    }

    #[test]
    fn test_update_same_path_different_resource() {
        let mut tracker = FileResourceTracker::new();
        let path = PathBuf::from("/conf/resource.yaml");

        // Track first resource
        tracker.track(path.clone(), "Gateway", "default/gw1", 111);
        assert!(tracker.has_key("Gateway", "default/gw1"));

        // Update same path with different resource
        tracker.track(path.clone(), "HTTPRoute", "default/route1", 222);

        // Old resource should be gone
        assert!(!tracker.has_key("Gateway", "default/gw1"));
        // New resource should be tracked
        assert!(tracker.has_key("HTTPRoute", "default/route1"));
        assert_eq!(tracker.get_by_path(&path).unwrap().0, "HTTPRoute");
    }

    #[test]
    fn test_same_resource_different_path() {
        let mut tracker = FileResourceTracker::new();
        let path1 = PathBuf::from("/conf/gw1.yaml");
        let path2 = PathBuf::from("/conf/gw2.yaml");

        // Track resource in path1
        tracker.track(path1.clone(), "Gateway", "default/my-gw", 111);
        assert!(tracker.has_path(&path1));

        // Track same resource in path2 (moved file)
        tracker.track(path2.clone(), "Gateway", "default/my-gw", 222);

        // Old path should be removed
        assert!(!tracker.has_path(&path1));
        // New path should be tracked
        assert!(tracker.has_path(&path2));
        // Resource should point to new path
        assert_eq!(
            tracker.get_path_by_key("Gateway", "default/my-gw").unwrap(),
            &path2
        );
    }

    #[test]
    fn test_update_hash() {
        let mut tracker = FileResourceTracker::new();
        let path = PathBuf::from("/conf/test.yaml");

        tracker.track(path.clone(), "Gateway", "default/gw", 100);
        assert_eq!(tracker.get_hash(&path), Some(100));

        assert!(tracker.update_hash(&path, 200));
        assert_eq!(tracker.get_hash(&path), Some(200));

        // Update non-existent path
        let other = PathBuf::from("/conf/other.yaml");
        assert!(!tracker.update_hash(&other, 300));
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut tracker = FileResourceTracker::new();
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);

        tracker.track(PathBuf::from("/a.yaml"), "Gateway", "default/a", 1);
        assert!(!tracker.is_empty());
        assert_eq!(tracker.len(), 1);

        tracker.track(PathBuf::from("/b.yaml"), "HTTPRoute", "default/b", 2);
        assert_eq!(tracker.len(), 2);

        tracker.clear();
        assert!(tracker.is_empty());
    }
}
