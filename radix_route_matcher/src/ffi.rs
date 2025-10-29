//! FFI bindings to the C rax library
//!
//! This module contains the low-level FFI declarations and internal wrapper functions
//! for interfacing with the C implementation of the Radix Tree.

use libc::{c_int, c_uchar, c_ulong, c_void};
use std::ptr;

/// Size of the static buffer in RaxIterator for small keys
const RAX_ITER_STATIC_LEN: usize = 128;
/// Size of the static stack in RaxStack
const RAX_STACK_STATIC_ITEMS: usize = 32;

/// Opaque type representing the Rax tree structure
#[repr(C)]
pub struct Rax {
    _private: [u8; 0],
}

/// Opaque type representing a node in the Rax tree
#[repr(C)]
pub struct RaxNode {
    _private: [u8; 0],
}

/// Callback function type for node operations
pub type RaxNodeCallback = Option<unsafe extern "C" fn(*mut *mut RaxNode) -> c_int>;

/// Stack structure for tracking tree traversal
#[repr(C)]
pub struct RaxStack {
    stack: *mut *mut c_void,
    items: usize,
    maxitems: usize,
    static_items: [*mut c_void; RAX_STACK_STATIC_ITEMS],
    oom: c_int,
}

/// Iterator structure for traversing the Rax tree
#[repr(C)]
pub struct RaxIterator {
    pub flags: c_int,
    pub rt: *mut Rax,
    pub key: *mut c_uchar,
    pub data: *mut c_void,
    pub key_len: usize,
    pub key_max: usize,
    pub key_static_string: [c_uchar; RAX_ITER_STATIC_LEN],
    pub node: *mut RaxNode,
    pub stack: RaxStack,
    pub node_cb: RaxNodeCallback,
}

// External C functions from rax.c
extern "C" {
    pub fn raxNew() -> *mut Rax;
    pub fn raxFree(rax: *mut Rax);
    pub fn raxInsert(
        rax: *mut Rax,
        s: *const c_uchar,
        len: c_ulong,
        data: *mut c_void,
        old: *mut *mut c_void,
    ) -> c_int;
    pub fn raxRemove(
        rax: *mut Rax,
        s: *const c_uchar,
        len: c_ulong,
        old: *mut *mut c_void,
    ) -> c_int;
    pub fn raxFind(rax: *mut Rax, s: *const c_uchar, len: c_ulong) -> *mut c_void;
    pub fn raxStart(it: *mut RaxIterator, rax: *mut Rax);
    pub fn raxSeek(
        it: *mut RaxIterator,
        op: *const c_uchar,
        ele: *const c_uchar,
        len: c_ulong,
    ) -> c_int;
    pub fn raxNext(it: *mut RaxIterator) -> c_int;
    pub fn raxPrev(it: *mut RaxIterator) -> c_int;
    pub fn raxUp(it: *mut RaxIterator) -> c_int;
    pub fn raxStop(it: *mut RaxIterator);
    pub static mut raxNotFound: *mut c_void;
}

// Internal wrapper functions
pub unsafe fn tree_new_raw() -> *mut c_void {
    raxNew() as *mut c_void
}

pub unsafe fn tree_destroy_raw(tree: *mut c_void) -> c_int {
    if tree.is_null() {
        return 0;
    }
    raxFree(tree as *mut Rax);
    0
}

pub unsafe fn tree_insert_raw(tree: *mut c_void, buf: *const u8, len: usize, idx: i32) -> c_int {
    if tree.is_null() {
        return -1;
    }
    if buf.is_null() {
        return -2;
    }
    let data = idx as isize as *mut c_void;
    raxInsert(
        tree as *mut Rax,
        buf as *const c_uchar,
        len as c_ulong,
        data,
        ptr::null_mut(),
    )
}

pub unsafe fn tree_find_raw(tree: *mut c_void, buf: *const u8, len: usize) -> *mut c_void {
    if tree.is_null() || buf.is_null() {
        return ptr::null_mut();
    }
    let res = raxFind(tree as *mut Rax, buf as *const c_uchar, len as c_ulong);
    if res == raxNotFound {
        ptr::null_mut()
    } else {
        res
    }
}

pub unsafe fn tree_remove_raw(tree: *mut c_void, buf: *const u8, len: usize) -> c_int {
    if tree.is_null() {
        return -1;
    }
    if buf.is_null() {
        return -2;
    }
    raxRemove(
        tree as *mut Rax,
        buf as *const c_uchar,
        len as c_ulong,
        ptr::null_mut(),
    )
}

pub unsafe fn tree_new_it_raw(tree: *mut c_void) -> *mut c_void {
    if tree.is_null() {
        return ptr::null_mut();
    }
    use std::mem;
    let size = mem::size_of::<RaxIterator>();
    let iter_ptr = libc::malloc(size) as *mut RaxIterator;
    if iter_ptr.is_null() {
        return ptr::null_mut();
    }
    raxStart(iter_ptr, tree as *mut Rax);
    iter_ptr as *mut c_void
}

pub unsafe fn tree_search_raw(
    _tree: *mut c_void,
    iter: *mut c_void,
    buf: *const u8,
    len: usize,
) -> *mut c_void {
    if iter.is_null() || buf.is_null() {
        return ptr::null_mut();
    }
    static OP_LE: [c_uchar; 3] = [b'<', b'=', 0];
    let iter_ptr = iter as *mut RaxIterator;
    raxSeek(
        iter_ptr,
        OP_LE.as_ptr(),
        buf as *const c_uchar,
        len as c_ulong,
    );
    iter
}

pub unsafe fn tree_up_raw(iter: *mut c_void, buf: *const u8, len: usize) -> c_int {
    if iter.is_null() || buf.is_null() {
        return -1;
    }
    let iter_ptr = iter as *mut RaxIterator;
    loop {
        let res = raxUp(iter_ptr);
        if res == 0 {
            return -1;
        }
        let key_len = (*iter_ptr).key_len;
        if key_len > len {
            continue;
        }
        let cmp = libc::memcmp(
            buf as *const c_void,
            (*iter_ptr).key as *const c_void,
            key_len,
        );
        if cmp != 0 {
            continue;
        }
        return (*iter_ptr).data as isize as c_int;
    }
}

pub unsafe fn tree_stop_raw(iter: *mut c_void) -> c_int {
    if iter.is_null() {
        return 0;
    }
    raxStop(iter as *mut RaxIterator);
    0
}
