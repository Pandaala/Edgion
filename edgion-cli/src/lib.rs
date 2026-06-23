// The `allocator-*` features are mutually exclusive; enabling more than one
// defines multiple `#[global_allocator]` statics. Fail the build early with an
// intentional message instead of a confusing downstream error.
#[cfg(any(
    all(feature = "allocator-jemalloc", feature = "allocator-mimalloc"),
    all(feature = "allocator-jemalloc", feature = "allocator-system"),
    all(feature = "allocator-mimalloc", feature = "allocator-system"),
))]
compile_error!(
    "features `allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` are mutually exclusive; enable exactly one"
);

pub mod certs;
pub mod cli;

pub use cli::Cli;
