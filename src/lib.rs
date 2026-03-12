// ==============================================
// Global Memory Allocator Configuration
// ==============================================

// Use jemalloc as the global allocator (default)
#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
use tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// Use mimalloc as the global allocator (optional)
#[cfg(feature = "allocator-mimalloc")]
use mimalloc::MiMalloc;

#[cfg(feature = "allocator-mimalloc")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

// Utility function to get allocator name
pub fn allocator_name() -> &'static str {
    #[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
    {
        "jemalloc"
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

// ==============================================
// Existing code
// ==============================================

pub mod core;
pub mod types;

pub use crate::core::controller::EdgionControllerCli;
pub use crate::core::gateway::EdgionGatewayCli;
