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
mod tests {
    use super::*;

    #[test]
    fn instances_and_generations_share_global_task_and_body_limits() {
        let global = MirrorBudget::new(1, 8).unwrap();
        let first = global.body_budget(8);
        let second = global.body_budget(8);
        let task = global.try_acquire_task().unwrap();
        assert!(global.try_acquire_task().is_none());
        let data = first.try_copy(b"12345678").unwrap();
        assert!(second.try_copy(b"x").is_none());
        drop(task);
        assert!(global.try_acquire_task().is_some());
        drop(data);
        assert!(second.try_copy(b"12345678").is_some());
    }

    #[test]
    fn last_transport_clone_owns_the_full_allocation_charge() {
        let budget = MirrorBodyBudget::for_test(8);
        let data = budget.try_copy(b"12345678").unwrap();
        let slice = data.slice(..1);
        drop(data);
        assert!(budget.try_copy(b"x").is_none());
        drop(slice);
        assert!(budget.try_copy(b"12345678").is_some());
    }

    #[test]
    fn failed_local_admission_returns_global_permits() {
        let global = MirrorBudget::new(1, 8).unwrap();
        let small = global.body_budget(1);
        assert!(small.try_copy(b"12").is_none());
        assert_eq!(global.bytes.available_permits(), 8);
        assert!(small.try_copy(b"1").is_some());
    }

    #[test]
    fn unsafe_direct_process_capacities_return_errors() {
        assert!(MirrorBudget::new(0, 1).is_err());
        assert!(MirrorBudget::new(1, 0).is_err());
        assert!(MirrorBudget::new(usize::MAX, 1).is_err());
        assert!(MirrorBudget::new(1, usize::MAX).is_err());
    }
}
