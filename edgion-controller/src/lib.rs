// Mutually exclusive allocator invariants. Fail the build early with an
// intentional message instead of emitting duplicate-global errors.
#[cfg(any(
    all(feature = "allocator-jemalloc", feature = "allocator-mimalloc"),
    all(feature = "allocator-jemalloc", feature = "allocator-system"),
    all(feature = "allocator-mimalloc", feature = "allocator-system"),
))]
compile_error!(
    "features `allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` are mutually exclusive; enable exactly one"
);

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

pub mod api;
pub mod authz;
pub mod cli;
pub mod conf_mgr;
pub mod conf_sync;
pub mod config;
pub mod fed_sync;
pub mod observe;
pub mod services;

pub use cli::EdgionControllerCli;

#[cfg(test)]
#[path = "../tests/unit/lib_test/reference_test_support.rs"]
mod reference_test_support;
