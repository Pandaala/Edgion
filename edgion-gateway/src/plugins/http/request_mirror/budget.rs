//! Process and per-mirror admission. Permits follow retained data, not queue slots.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::config::{RequestMirrorGlobalConfig, ResolvedRequestMirrorGlobalConfig};

static GLOBAL_BUDGET: OnceLock<Arc<MirrorBudget>> = OnceLock::new();

pub fn init_global_budget(config: &ResolvedRequestMirrorGlobalConfig) -> Result<(), String> {
    let budget = MirrorBudget::new(config.max_concurrent, config.max_buffered_bytes)?;
    GLOBAL_BUDGET
        .set(budget)
        .map_err(|_| "RequestMirror process budget is already initialized".to_string())
}

/// Called when compiling a plugin, never lazily on the request path.
pub(super) fn global_budget() -> Arc<MirrorBudget> {
    Arc::clone(GLOBAL_BUDGET.get_or_init(|| {
        let defaults = RequestMirrorGlobalConfig::default();
        // Fixed defaults are checked independently by the configuration tests.
        let config = defaults.resolve().expect("valid RequestMirror defaults");
        MirrorBudget::new(config.max_concurrent, config.max_buffered_bytes).expect("valid RequestMirror budget")
    }))
}

pub(super) struct MirrorBudget {
    tasks: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}

impl MirrorBudget {
    pub(super) fn new(tasks: usize, bytes: usize) -> Result<Arc<Self>, String> {
        if tasks == 0
            || tasks > Semaphore::MAX_PERMITS
            || bytes == 0
            || bytes > u32::MAX as usize
            || bytes > Semaphore::MAX_PERMITS
        {
            return Err("invalid RequestMirror process budget".to_string());
        }
        Ok(Arc::new(Self {
            tasks: Arc::new(Semaphore::new(tasks)),
            bytes: Arc::new(Semaphore::new(bytes)),
        }))
    }

    pub(super) fn try_acquire_task(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.tasks).try_acquire_owned().ok()
    }

    pub(super) fn try_reserve_snapshot(&self, bytes: usize) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.bytes)
            .try_acquire_many_owned(u32::try_from(bytes).ok()?)
            .ok()
    }

    pub(super) fn body_budget(&self, bytes: usize) -> MirrorBodyBudget {
        MirrorBodyBudget {
            global: Arc::clone(&self.bytes),
            local: Arc::new(Semaphore::new(bytes)),
        }
    }
}

/// Outstanding body data for one mirror, including data retained by the transport.
#[derive(Clone)]
pub struct MirrorBodyBudget {
    global: Arc<Semaphore>,
    local: Arc<Semaphore>,
}

pub(super) struct BodyLease {
    _global: OwnedSemaphorePermit,
    _local: OwnedSemaphorePermit,
}

impl MirrorBodyBudget {
    pub(super) fn try_reserve(&self, len: usize) -> Option<BodyLease> {
        let len = u32::try_from(len).ok()?;
        let global = Arc::clone(&self.global).try_acquire_many_owned(len).ok()?;
        let local = Arc::clone(&self.local).try_acquire_many_owned(len).ok()?;
        Some(BodyLease {
            _global: global,
            _local: local,
        })
    }

    /// Own exactly the copied payload. Cloning a borrowed `Bytes` slice could
    /// retain a much larger allocation that its visible length cannot account for.
    pub(crate) fn try_copy(&self, data: &[u8]) -> Option<Bytes> {
        if data.is_empty() {
            return Some(Bytes::new());
        }
        let lease = self.try_reserve(data.len())?;
        Some(Bytes::from_owner(BudgetedBytes {
            data: data.to_vec().into_boxed_slice(),
            _lease: lease,
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test(bytes: usize) -> Self {
        MirrorBudget::new(1, bytes).unwrap().body_budget(bytes)
    }
}

struct BudgetedBytes {
    // Data must be dropped before its permits are returned.
    data: Box<[u8]>,
    _lease: BodyLease,
}

impl AsRef<[u8]> for BudgetedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/request_mirror/budget_test.rs"]
mod tests;
