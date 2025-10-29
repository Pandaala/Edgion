//! C ABI exports
//!
//! This module exports C-compatible functions that can be called from C/C++ or other languages
//! through FFI. These functions provide the same functionality as the Rust API but with a C ABI.

use crate::ffi::*;
use libc::{c_int, c_uchar, c_ulong, c_void};

/// Creates a new radix tree.
///
/// # Returns
///
/// Returns a pointer to the new tree, or NULL on failure.
///
/// # Safety
///
/// The returned pointer must be freed with radix_tree_destroy().
#[no_mangle]
pub extern "C" fn radix_tree_new() -> *mut c_void {
    unsafe { tree_new_raw() }
}

/// Destroys a radix tree and frees all associated memory.
///
/// # Arguments
///
/// * t - Pointer to the tree to destroy
///
/// # Returns
///
/// Returns 0 on success.
///
/// # Safety
///
/// The pointer must have been returned by radix_tree_new().
/// After calling this function, the pointer is invalid and must not be used.
#[no_mangle]
pub extern "C" fn radix_tree_destroy(t: *mut c_void) -> c_int {
    unsafe { tree_destroy_raw(t) }
}

/// Inserts a key-value pair into the tree.
///
/// # Arguments
///
/// * t - Pointer to the tree
/// * buf - Pointer to the key data
/// * len - Length of the key in bytes
/// * idx - The integer value to associate with the key
///
/// # Returns
///
/// Returns 0 on success, negative on error.
///
/// # Safety
///
/// t must be a valid tree pointer, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_insert(
    t: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
    idx: c_int,
) -> c_int {
    unsafe { tree_insert_raw(t, buf as *const u8, len as usize, idx) }
}

/// Finds an exact match for a key.
///
/// # Arguments
///
/// * t - Pointer to the tree
/// * buf - Pointer to the key data
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns the associated value cast to a pointer, or NULL if not found.
///
/// # Safety
///
/// t must be a valid tree pointer, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_find(
    t: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> *mut c_void {
    unsafe { tree_find_raw(t, buf as *const u8, len as usize) }
}

/// Removes a key from the tree.
///
/// # Arguments
///
/// * t - Pointer to the tree
/// * buf - Pointer to the key data
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns 0 on success, negative if key not found or on error.
///
/// # Safety
///
/// t must be a valid tree pointer, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_remove(
    t: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> c_int {
    unsafe { tree_remove_raw(t, buf as *const u8, len as usize) }
}

/// Creates a new iterator for the tree.
///
/// # Arguments
///
/// * t - Pointer to the tree
///
/// # Returns
///
/// Returns a pointer to the new iterator, or NULL on failure.
///
/// # Safety
///
/// t must be a valid tree pointer.
/// The returned iterator must be freed with libc::free() after calling radix_tree_stop().
#[no_mangle]
pub extern "C" fn radix_tree_new_it(t: *mut c_void) -> *mut c_void {
    unsafe { tree_new_it_raw(t) }
}

/// Initializes an iterator for prefix searching.
///
/// # Arguments
///
/// * tree - Pointer to the tree
/// * it - Pointer to the iterator
/// * buf - Pointer to the key data to search for
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns the iterator pointer on success, NULL on failure.
///
/// # Safety
///
/// All pointers must be valid, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_search(
    tree: *mut c_void,
    it: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> *mut c_void {
    unsafe { tree_search_raw(tree, it, buf as *const u8, len as usize) }
}

/// Moves to the previous matching prefix.
///
/// # Arguments
///
/// * it - Pointer to the iterator
/// * buf - Pointer to the key data being searched
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns the associated value on success, -1 if no more matches.
///
/// # Safety
///
/// it must be a valid iterator, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_prev(
    it: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> c_int {
    if it.is_null() || buf.is_null() {
        return -1;
    }
    let iter_ptr = it as *mut RaxIterator;
    loop {
        let res = unsafe { crate::ffi::raxPrev(iter_ptr) };
        if res == 0 {
            return -1;
        }
        let key_len = unsafe { (*iter_ptr).key_len };
        if key_len > len as usize {
            continue;
        }
        let cmp = unsafe {
            libc::memcmp(
                buf as *const c_void,
                (*iter_ptr).key as *const c_void,
                key_len,
            )
        };
        if cmp != 0 {
            continue;
        }
        return unsafe { (*iter_ptr).data as isize as c_int };
    }
}

/// Moves to the next matching key.
///
/// # Arguments
///
/// * it - Pointer to the iterator
/// * buf - Pointer to the key data being searched
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns the associated value on success, -1 if no more matches.
///
/// # Safety
///
/// it must be a valid iterator, buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_next(
    it: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> c_int {
    if it.is_null() || buf.is_null() {
        return -1;
    }
    let iter_ptr = it as *mut RaxIterator;
    let res = unsafe { crate::ffi::raxNext(iter_ptr) };
    if res == 0 {
        return -1;
    }
    let key_len = unsafe { (*iter_ptr).key_len };
    if key_len > len as usize {
        return -1;
    }
    let cmp = unsafe {
        libc::memcmp(
            buf as *const c_void,
            (*iter_ptr).key as *const c_void,
            key_len,
        )
    };
    if cmp != 0 {
        return -1;
    }
    unsafe { (*iter_ptr).data as isize as c_int }
}

/// Moves iterator up to find the next shorter prefix match.
///
/// # Arguments
///
/// * it - Pointer to the iterator
/// * buf - Pointer to the key data being searched
/// * len - Length of the key in bytes
///
/// # Returns
///
/// Returns the associated value on success, -1 if no more matches.
///
/// # Safety
///
/// it must be a valid iterator initialized with radix_tree_search(),
/// buf must point to at least len bytes.
#[no_mangle]
pub extern "C" fn radix_tree_up(
    it: *mut c_void,
    buf: *const c_uchar,
    len: c_ulong,
) -> c_int {
    unsafe { tree_up_raw(it, buf as *const u8, len as usize) }
}

/// Stops an iterator and releases its internal resources.
///
/// # Arguments
///
/// * it - Pointer to the iterator
///
/// # Returns
///
/// Returns 0 on success.
///
/// # Safety
///
/// it must be a valid iterator.
/// After calling this, the iterator memory must still be freed with libc::free().
#[no_mangle]
pub extern "C" fn radix_tree_stop(it: *mut c_void) -> c_int {
    unsafe { tree_stop_raw(it) }
}
