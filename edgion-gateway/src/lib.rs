// Mutually exclusive feature invariants. These selections wire incompatible
// backends (multiple `#[global_allocator]` statics, conflicting pingora TLS
// backends), so fail the build early with an intentional message instead of
// emitting confusing downstream errors (e.g. via `--all-features`).
#[cfg(any(
    all(feature = "allocator-jemalloc", feature = "allocator-mimalloc"),
    all(feature = "allocator-jemalloc", feature = "allocator-system"),
    all(feature = "allocator-mimalloc", feature = "allocator-system"),
))]
compile_error!(
    "features `allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` are mutually exclusive; enable exactly one"
);

#[cfg(all(feature = "boringssl", feature = "rustls"))]
compile_error!("features `boringssl` and `rustls` are mutually exclusive; enable exactly one TLS backend");

#[cfg(test)]
pub(crate) mod test_current_status;

#[cfg(test)]
mod test_allocation_counter {
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
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: test_allocation_counter::CountingAllocator = test_allocation_counter::CountingAllocator;

/// Name of the active global memory allocator (selected by the binary's
/// `allocator-*` feature). Reported in startup/version output.
pub fn allocator_name() -> &'static str {
    #[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
    {
        "jemalloc"
    }

    #[cfg(all(feature = "allocator-jemalloc", target_env = "msvc"))]
    {
        "system (default)"
    }

    #[cfg(feature = "allocator-mimalloc")]
    {
        "mimalloc"
    }

    #[cfg(feature = "allocator-system")]
    {
        "system"
    }

    #[cfg(not(any(
        feature = "allocator-jemalloc",
        feature = "allocator-mimalloc",
        feature = "allocator-system"
    )))]
    {
        "system (default)"
    }
}

/// Stable, line-oriented description of compile-time Gateway capabilities.
///
/// Integration scripts consume this output before starting the process. Keep
/// the format Bash 3.2-friendly and bump the version before incompatible edits.
/// The Gateway always builds against the Edgion Pingora fork, so the body
/// capture seam is unconditionally present.
pub fn build_capabilities() -> &'static str {
    "format=edgion-build-capabilities/v1\nbody-buffering=enabled\npingora-mode=edgion-fork\n"
}

pub mod ai;
pub mod api;
pub mod backends;
pub mod cache;
pub mod cli;
pub mod conf_sync;
pub mod config;
pub mod ctx;
pub mod geoip;
pub mod lb;
pub mod link_sys;
pub mod observe;
pub mod plugins;
pub mod request_body;
mod response_processor_driver;
pub mod routes;
pub mod runtime;
pub mod services;
pub mod sse;
pub mod tls;
pub mod util;

pub use ctx::{
    extract_log_labels, BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, ExternalJumpPreset,
    InternalJumpPreset, LogLabels, MatchInfo, MatchedInfo, MirrorConfig, MirrorState, PeerTimeoutSource,
    PendingHeaderOp, RequestInfo, ResponseStageDenial, Rsc, SessionPersistenceState, SetJumpOutcome, TlsConnId,
    TlsConnMeta, UpstreamInfo, UpstreamJumpTarget,
};

pub use cli::EdgionGatewayCli;
pub(crate) use runtime::send_grpc_trailers_only;
pub use runtime::{
    end_response_400, end_response_404, end_response_421, end_response_500, end_response_502, end_response_503,
    end_response_grpc_unavailable, end_response_grpc_unimplemented, terminate_with_fallback,
};

pub mod request_trace_id;

#[cfg(test)]
mod request_id_tests;

#[cfg(test)]
mod test_subprocess;
