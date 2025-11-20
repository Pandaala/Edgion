//! High-level Rust API for the Radix Tree
//!
//! This model provides a safe, idiomatic Rust interface to the underlying C implementation.

use crate::ffi::*;
use libc::c_void;
use std::ptr;

/// A high-level Rust wrapper for the Radix Tree data structure.
///
/// `RadixTree` provides efficient storage and retrieval of string keys with associated
/// integer values. It supports exact matching, prefix matching, and iteration.
///
/// # Examples
///
/// ```
/// use radix_route_matcher::RadixTree;
///
/// let mut tree = RadixTree::new().unwrap();
/// tree.insert("/api/users", 1).unwrap();
/// tree.insert("/api/posts", 2).unwrap();
///
/// assert_eq!(tree.find_exact("/api/users"), Some(1));
///
/// let iter = tree.create_iter().unwrap();
/// assert_eq!(tree.longest_prefix(&iter, "/api/users/123"), Some(1));
/// ```
pub struct RadixTree {
    tree: *mut c_void,
}

/// Iterator for RadixTree operations.
///
/// This is a lightweight structure that can be created on-demand for tree traversal.
/// Each iterator is independent, allowing multiple concurrent read operations on the
/// same tree from different threads.
///
/// # Thread Safety
///
/// Creating separate iterators for each query enables lock-free concurrent reads.
pub struct RadixIterator {
    iter: *mut c_void,
}

impl RadixTree {
    /// Creates a new empty Radix Tree.
    ///
    /// # Errors
    ///
    /// Returns an error if memory allocation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let tree = RadixTree::new().unwrap();
    /// ```
    pub fn new() -> Result<Self, &'static str> {
        let tree = unsafe { tree_new_raw() };
        if tree.is_null() {
            return Err("failed to allocate radix tree");
        }

        Ok(Self { tree })
    }

    /// Creates a new iterator for this tree.
    ///
    /// Iterators are lightweight and can be created on-demand for each query.
    /// This enables lock-free concurrent reads on the same tree.
    ///
    /// # Errors
    ///
    /// Returns an error if iterator allocation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let tree = RadixTree::new().unwrap();
    /// let iter = tree.create_iter().unwrap();
    /// ```
    pub fn create_iter(&self) -> Result<RadixIterator, &'static str> {
        let iter = unsafe { tree_new_it_raw(self.tree) };
        if iter.is_null() {
            return Err("failed to allocate radix tree iterator");
        }
        Ok(RadixIterator { iter })
    }

    /// Inserts a path with an associated index into the tree.
    ///
    /// If the path already exists, its value will be updated.
    ///
    /// # Arguments
    ///
    /// * `path` - The path string to insert
    /// * `idx` - The integer index to associate with this path (must be > 0)
    ///
    /// # Errors
    ///
    /// Returns an error code if the insertion fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/api", 1).unwrap();
    /// tree.insert("/api/users", 2).unwrap();
    /// ```
    pub fn insert(&mut self, path: &str, idx: i32) -> Result<(), i32> {
        let bytes = path.as_bytes();
        let rc = unsafe { tree_insert_raw(self.tree, bytes.as_ptr(), bytes.len(), idx) };
        if rc < 0 {
            Err(rc)
        } else {
            Ok(())
        }
    }

    /// Finds the exact match for a path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to search for
    ///
    /// # Returns
    ///
    /// Returns `Some(idx)` if the path exists, `None` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/api", 1).unwrap();
    ///
    /// assert_eq!(tree.find_exact("/api"), Some(1));
    /// assert_eq!(tree.find_exact("/api/users"), None);
    /// ```
    pub fn find_exact(&self, path: &str) -> Option<i32> {
        let bytes = path.as_bytes();
        let res = unsafe { tree_find_raw(self.tree, bytes.as_ptr(), bytes.len()) };
        if res.is_null() {
            None
        } else {
            Some(res as isize as i32)
        }
    }

    /// Removes a path from the tree.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to remove
    ///
    /// # Errors
    ///
    /// Returns an error code if the path doesn't exist or removal fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/api", 1).unwrap();
    /// tree.remove("/api").unwrap();
    /// assert_eq!(tree.find_exact("/api"), None);
    /// ```
    pub fn remove(&mut self, path: &str) -> Result<(), i32> {
        let bytes = path.as_bytes();
        let rc = unsafe { tree_remove_raw(self.tree, bytes.as_ptr(), bytes.len()) };
        if rc < 0 {
            Err(rc)
        } else {
            Ok(())
        }
    }

    /// Finds the longest prefix match for a path.
    ///
    /// This is useful for route matching where you want to find the most specific
    /// route that matches the beginning of the given path.
    ///
    /// # Arguments
    ///
    /// * `iter` - A RadixIterator for this tree
    /// * `path` - The path to match
    ///
    /// # Returns
    ///
    /// Returns `Some(idx)` of the longest matching prefix, `None` if no match.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/api", 1).unwrap();
    /// tree.insert("/api/users", 2).unwrap();
    ///
    /// let iter = tree.create_iter().unwrap();
    /// // Matches "/api/users" (idx=2)
    /// assert_eq!(tree.longest_prefix(&iter, "/api/users/123"), Some(2));
    /// ```
    pub fn longest_prefix(&self, iter: &RadixIterator, path: &str) -> Option<i32> {
        let bytes = path.as_bytes();
        let ptr = bytes.as_ptr();
        let len = bytes.len();

        let search_ptr = unsafe { tree_search_raw(self.tree, iter.iter, ptr, len) };
        if search_ptr.is_null() {
            return None;
        }

        let idx = unsafe { tree_up_raw(iter.iter, ptr, len) };
        if idx <= 0 {
            None
        } else {
            Some(idx)
        }
    }

    /// Initializes the iterator for prefix searching.
    ///
    /// Call this before calling `next_prefix()`.
    ///
    /// # Arguments
    ///
    /// * `iter` - A RadixIterator for this tree
    /// * `path` - The path to search prefixes for
    ///
    /// # Returns
    ///
    /// Returns `true` if initialization succeeded, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/api", 1).unwrap();
    ///
    /// let iter = tree.create_iter().unwrap();
    /// if tree.search(&iter, "/api/users") {
    ///     while let Some(idx) = tree.next_prefix(&iter, "/api/users") {
    ///         println!("Matched: {}", idx);
    ///     }
    /// }
    /// ```
    pub fn search(&self, iter: &RadixIterator, path: &str) -> bool {
        let bytes = path.as_bytes();
        let search_ptr = unsafe { tree_search_raw(self.tree, iter.iter, bytes.as_ptr(), bytes.len()) };
        !search_ptr.is_null()
    }

    /// Gets the next prefix match (from longest to shortest).
    ///
    /// Must call `search()` first to initialize the iterator.
    ///
    /// # Arguments
    ///
    /// * `iter` - A RadixIterator for this tree (same as passed to `search()`)
    /// * `path` - The path being searched (same as passed to `search()`)
    ///
    /// # Returns
    ///
    /// Returns `Some(idx)` for the next match, `None` when no more matches.
    ///
    /// # Examples
    ///
    /// See `search()` for example usage.
    pub fn next_prefix(&self, iter: &RadixIterator, path: &str) -> Option<i32> {
        let bytes = path.as_bytes();
        let idx = unsafe { tree_up_raw(iter.iter, bytes.as_ptr(), bytes.len()) };
        if idx <= 0 {
            None
        } else {
            Some(idx)
        }
    }

    /// Returns all matching prefixes for a path.
    ///
    /// This is a convenience method that combines `search()` and `next_prefix()`
    /// to return all matches at once, ordered from longest to shortest prefix.
    ///
    /// # Arguments
    ///
    /// * `iter` - A RadixIterator for this tree
    /// * `path` - The path to find prefixes for
    ///
    /// # Returns
    ///
    /// A vector of indices for all matching prefixes, from longest to shortest.
    ///
    /// # Examples
    ///
    /// ```
    /// use radix_route_matcher::RadixTree;
    ///
    /// let mut tree = RadixTree::new().unwrap();
    /// tree.insert("/", 1).unwrap();
    /// tree.insert("/api", 2).unwrap();
    /// tree.insert("/api/users", 3).unwrap();
    ///
    /// let iter = tree.create_iter().unwrap();
    /// let matches = tree.find_all_prefixes(&iter, "/api/users/123");
    /// assert_eq!(matches, vec![3, 2, 1]);
    /// ```
    pub fn find_all_prefixes(&self, iter: &RadixIterator, path: &str) -> Vec<i32> {
        let mut results = Vec::with_capacity(10);

        if !self.search(iter, path) {
            return results;
        }

        while let Some(idx) = self.next_prefix(iter, path) {
            results.push(idx);
        }

        results
    }
}

impl Drop for RadixTree {
    fn drop(&mut self) {
        unsafe {
            tree_destroy_raw(self.tree);
        }
        self.tree = ptr::null_mut();
    }
}

impl Drop for RadixIterator {
    fn drop(&mut self) {
        unsafe {
            tree_stop_raw(self.iter);
            if !self.iter.is_null() {
                libc::free(self.iter);
            }
        }
        self.iter = ptr::null_mut();
    }
}

// RadixTree is now thread-safe for concurrent reads (with separate iterators)
// The tree itself is immutable during reads, only modifications need &mut
unsafe impl Send for RadixTree {}
unsafe impl Sync for RadixTree {}

// RadixIterator is not thread-safe and should not be shared between threads
unsafe impl Send for RadixIterator {}
