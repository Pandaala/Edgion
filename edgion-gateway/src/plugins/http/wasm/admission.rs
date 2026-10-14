//! Process admission for guest allocations and source preparation.
//!
//! Guest bytes cover linear memory and table slots, not host payloads, compiler
//! heap, Engine virtual reservations or RSS. Charges outlive their Store.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_GUEST_MEMORY_BYTES: usize = 512 * 1024 * 1024;
const MAX_PREPARATIONS: usize = 4;
static BUDGET: OnceLock<Arc<GuestBudget>> = OnceLock::new();
static PREPARATIONS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub fn initialize(bytes: usize) -> Result<(), &'static str> {
    let budget = GuestBudget::new(bytes)?;
    BUDGET.set(budget).map_err(|_| "Wasm guest budget already initialized")
}

fn global_budget() -> Arc<GuestBudget> {
    BUDGET
        .get_or_init(|| GuestBudget::new(DEFAULT_GUEST_MEMORY_BYTES).expect("valid Wasm guest budget"))
        .clone()
}

pub(super) fn try_prepare() -> Result<Arc<OwnedSemaphorePermit>, String> {
    PREPARATIONS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_PREPARATIONS)))
        .clone()
        .try_acquire_owned()
        .map(Arc::new)
        .map_err(|_| "wasm preparation capacity exhausted".to_string())
}

/// Both the running job and its undelivered result retain admission. Dropping
/// an async waiter cannot abort a running blocking task or free its payloads.
pub(super) async fn prepare_blocking<T: Send + 'static>(
    permit: &Arc<OwnedSemaphorePermit>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let permit = permit.clone();
    let (result, _permit) = tokio::task::spawn_blocking(move || (work(), permit))
        .await
        .map_err(|_| "wasm preparation task failed".to_string())?;
    result
}

pub(super) struct GuestBudget {
    limit: usize,
    used: AtomicUsize,
}

impl GuestBudget {
    pub(super) fn new(limit: usize) -> Result<Arc<Self>, &'static str> {
        if limit == 0 || limit > isize::MAX as usize {
            return Err("wasm_guest_memory_bytes must be positive and fit the platform allocation range");
        }
        Ok(Arc::new(Self {
            limit,
            used: AtomicUsize::new(0),
        }))
    }

    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|total| *total <= self.limit)
            })
            .is_ok()
    }
}

/// The Store's limiter and its external destruction guard share this charge.
/// It is released only after all owners, including the external guard, drop.
pub(super) struct GuestMemory {
    budget: Arc<GuestBudget>,
    bytes: AtomicUsize,
}

impl Default for GuestMemory {
    fn default() -> Self {
        Self {
            budget: global_budget(),
            bytes: AtomicUsize::new(0),
        }
    }
}

impl GuestMemory {
    pub(super) fn reserve(&self, bytes: usize) -> bool {
        if !self.budget.reserve(bytes) {
            return false;
        }
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
        true
    }

    pub(super) fn rollback(&self, bytes: usize) {
        self.bytes.fetch_sub(bytes, Ordering::AcqRel);
        self.budget.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(*self.bytes.get_mut(), Ordering::AcqRel);
    }
}

#[derive(Serialize)]
pub struct Stats {
    guest_bytes: usize,
    guest_limit_bytes: usize,
    preparations: usize,
}

pub fn stats() -> Stats {
    Stats {
        guest_bytes: BUDGET.get().map_or(0, |b| b.used.load(Ordering::Acquire)),
        guest_limit_bytes: BUDGET.get().map_or(DEFAULT_GUEST_MEMORY_BYTES, |b| b.limit),
        preparations: PREPARATIONS
            .get()
            .map_or(0, |s| MAX_PREPARATIONS - s.available_permits()),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/admission_test.rs"]
mod tests;
