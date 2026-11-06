//! Startup readiness deadline shared by every configuration-center backend.
//!
//! `cache_ready_deadline_secs` is the single bound on how long one resource
//! epoch may take to load every watched source and reach readiness. Phase waits
//! inside the epoch are deliberately unbounded so a large or slow source cannot
//! fail on a hidden inner deadline the operator cannot raise.

/// Default startup readiness deadline in seconds.
pub(crate) const DEFAULT_CACHE_READY_DEADLINE_SECS: u64 = 180;

/// Values at or below this floor are rejected at startup: a full initial load
/// that must finish this fast is a designed-in restart loop on any non-trivial
/// deployment.
pub(crate) const MIN_CACHE_READY_DEADLINE_SECS: u64 = 30;

pub(crate) fn default_cache_ready_deadline_secs() -> u64 {
    DEFAULT_CACHE_READY_DEADLINE_SECS
}

/// Fail-fast validation of `cache_ready_deadline_secs` for `backend`.
pub(crate) fn validate_cache_ready_deadline_secs(backend: &str, secs: u64) -> anyhow::Result<()> {
    if secs <= MIN_CACHE_READY_DEADLINE_SECS {
        anyhow::bail!(
            "{backend} conf_center misconfig: cache_ready_deadline_secs ({secs}) must be > \
             {MIN_CACHE_READY_DEADLINE_SECS}; a deadline too short for the initial load is a \
             designed-in restart loop"
        );
    }
    Ok(())
}
