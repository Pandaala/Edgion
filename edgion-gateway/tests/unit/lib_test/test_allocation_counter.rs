use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

pub struct CountingAllocator;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static REQUESTED_BYTES: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocationStats {
    pub calls: usize,
    pub requested_bytes: usize,
}

fn record_allocation(requested_bytes: usize) {
    ENABLED.with(|enabled| {
        if enabled.get() {
            CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
            REQUESTED_BYTES.with(|bytes| bytes.set(bytes.get().saturating_add(requested_bytes)));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: this wrapper forwards the caller-provided layout without
        // modification to the process system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr/layout came from the forwarded System allocation.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation(new_size);
        // SAFETY: ptr/layout came from System and new_size is forwarded.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

pub fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let (value, stats) = count_allocation_stats(f);
    (value, stats.calls)
}

pub fn count_allocation_stats<R>(f: impl FnOnce() -> R) -> (R, AllocationStats) {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ENABLED.with(|enabled| enabled.set(false));
        }
    }

    assert!(
        !ENABLED.with(Cell::get),
        "nested allocation measurements are not supported"
    );
    CALLS.with(|calls| calls.set(0));
    REQUESTED_BYTES.with(|bytes| bytes.set(0));
    ENABLED.with(|enabled| enabled.set(true));
    let guard = Guard;
    let value = f();
    drop(guard);
    let stats = AllocationStats {
        calls: CALLS.with(Cell::get),
        requested_bytes: REQUESTED_BYTES.with(Cell::get),
    };
    (value, stats)
}

#[test]
fn nested_measurement_is_rejected_without_poisoning_the_next_measurement() {
    let nested = std::panic::catch_unwind(|| count_allocation_stats(|| count_allocation_stats(|| ())));
    assert!(nested.is_err());

    let (_, stats) = count_allocation_stats(|| std::hint::black_box(Box::new(7_u8)));
    assert_eq!(stats.calls, 1);
    assert_eq!(stats.requested_bytes, 1);
}
