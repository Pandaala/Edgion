//! Process admission for guest allocations and source preparation.
//!
//! Guest bytes cover linear memory and table slots, not host payloads, compiler
//! heap, Engine virtual reservations or RSS. Charges outlive their Store.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

use serde::{Deserialize, Serialize};

use super::host_abi::{MAX_GUEST_MEMORY_BYTES, MAX_GUEST_TABLE_ELEMENTS, MAX_RETAINED_ABI_BYTES, MAX_RETAINED_ABI_OPS};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_GUEST_MEMORY_BYTES: usize = 512 * 1024 * 1024;
/// Restart-only process limits. Per-guest and ABI limits can only tighten the
/// reviewed safety envelopes; none of the positive limits supports unlimited.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct WasmLimits {
    pub guest_memory_bytes: usize,
    pub max_instances: usize,
    pub max_preparations: usize,
    pub max_instance_memory_bytes: usize,
    pub max_instance_table_elements: usize,
    pub max_idle_instances: usize,
    pub max_idle_guest_memory_bytes: usize,
    pub max_host_call_bytes: usize,
    pub max_host_abi_bytes: usize,
    pub max_host_abi_ops: usize,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            guest_memory_bytes: DEFAULT_GUEST_MEMORY_BYTES,
            max_instances: 128,
            max_preparations: 4,
            max_instance_memory_bytes: MAX_GUEST_MEMORY_BYTES,
            max_instance_table_elements: MAX_GUEST_TABLE_ELEMENTS,
            max_idle_instances: 32,
            max_idle_guest_memory_bytes: 128 * 1024 * 1024,
            max_host_call_bytes: MAX_GUEST_MEMORY_BYTES,
            max_host_abi_bytes: MAX_RETAINED_ABI_BYTES,
            max_host_abi_ops: MAX_RETAINED_ABI_OPS,
        }
    }
}

impl WasmLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.guest_memory_bytes == 0 || self.guest_memory_bytes > isize::MAX as usize {
            return Err("wasm_limits.guest_memory_bytes must be positive and fit the platform allocation range");
        }
        if !(1..=1024).contains(&self.max_instances) {
            return Err("wasm_limits.max_instances must be between 1 and 1024");
        }
        if !(1..=32).contains(&self.max_preparations) {
            return Err("wasm_limits.max_preparations must be between 1 and 32");
        }
        if !(1..=MAX_GUEST_MEMORY_BYTES).contains(&self.max_instance_memory_bytes) {
            return Err("wasm_limits.max_instance_memory_bytes must be between 1 and 134217728");
        }
        if !(1..=MAX_GUEST_TABLE_ELEMENTS).contains(&self.max_instance_table_elements) {
            return Err("wasm_limits.max_instance_table_elements must be between 1 and 20000");
        }
        if self.max_idle_instances > self.max_instances {
            return Err("wasm_limits.max_idle_instances must not exceed max_instances");
        }
        if self.max_idle_guest_memory_bytes > self.guest_memory_bytes {
            return Err("wasm_limits.max_idle_guest_memory_bytes must not exceed guest_memory_bytes");
        }
        if !(1..=MAX_GUEST_MEMORY_BYTES).contains(&self.max_host_call_bytes) {
            return Err("wasm_limits.max_host_call_bytes must be between 1 and 134217728");
        }
        if !(1..=MAX_RETAINED_ABI_BYTES).contains(&self.max_host_abi_bytes) {
            return Err("wasm_limits.max_host_abi_bytes must be between 1 and 33554432");
        }
        if !(1..=MAX_RETAINED_ABI_OPS).contains(&self.max_host_abi_ops) {
            return Err("wasm_limits.max_host_abi_ops must be between 1 and 200000");
        }
        Ok(())
    }
}

struct Admission {
    limits: WasmLimits,
    guest: Arc<GuestBudget>,
    instances: Arc<Semaphore>,
    preparations: Arc<Semaphore>,
    idle_slots: Arc<Semaphore>,
    idle_bytes: Arc<GuestBudget>,
}

impl Admission {
    fn new(limits: WasmLimits) -> Result<Self, &'static str> {
        limits.validate()?;
        Ok(Self {
            guest: GuestBudget::new(limits.guest_memory_bytes)?,
            instances: Arc::new(Semaphore::new(limits.max_instances)),
            preparations: Arc::new(Semaphore::new(limits.max_preparations)),
            idle_slots: Arc::new(Semaphore::new(limits.max_idle_instances)),
            idle_bytes: Arc::new(GuestBudget {
                limit: limits.max_idle_guest_memory_bytes,
                used: AtomicUsize::new(0),
            }),
            limits,
        })
    }

    fn try_idle(&self, bytes: usize) -> Option<IdleReservation> {
        // Zero bytes disables retention even for guests without linear memory.
        if self.limits.max_idle_guest_memory_bytes == 0 {
            return None;
        }
        let slot = self.idle_slots.clone().try_acquire_owned().ok()?;
        if !self.idle_bytes.reserve(bytes) {
            return None;
        }
        Some(IdleReservation {
            budget: self.idle_bytes.clone(),
            bytes,
            _slot: slot,
        })
    }

    fn stats(&self) -> Stats {
        Stats {
            limits: self.limits,
            guest_bytes: self.guest.used.load(Ordering::Acquire),
            instances: self.limits.max_instances - self.instances.available_permits(),
            preparations: self.limits.max_preparations - self.preparations.available_permits(),
            idle_instances: self.limits.max_idle_instances - self.idle_slots.available_permits(),
            idle_guest_bytes: self.idle_bytes.used.load(Ordering::Acquire),
        }
    }
}

// Publish limits and every associated capacity together, never a partial setup.
static ADMISSION: OnceLock<Admission> = OnceLock::new();

pub fn initialize(limits: WasmLimits) -> Result<(), &'static str> {
    let admission = Admission::new(limits)?;
    ADMISSION
        .set(admission)
        .map_err(|_| "Wasm admission already initialized")
}

fn global() -> &'static Admission {
    ADMISSION
        .get_or_init(|| Admission::new(default_process_limits()).expect("default Wasm limits satisfy safety bounds"))
}

#[cfg(not(test))]
fn default_process_limits() -> WasmLimits {
    WasmLimits::default()
}

/// The lib test harness runs many independent loads in one process; the
/// production preparation bound (4, non-waiting) would make them reject each
/// other. Tests that exercise exhaustion read `limits()` instead of assuming 4.
#[cfg(test)]
fn default_process_limits() -> WasmLimits {
    WasmLimits {
        max_preparations: 32,
        ..WasmLimits::default()
    }
}

pub(super) fn limits() -> &'static WasmLimits {
    &global().limits
}

pub(super) fn instance_permits() -> &'static Arc<Semaphore> {
    &global().instances
}

pub(super) fn try_prepare() -> Result<Arc<OwnedSemaphorePermit>, String> {
    global()
        .preparations
        .clone()
        .try_acquire_owned()
        .map(Arc::new)
        .map_err(|_| "wasm preparation capacity exhausted".to_string())
}

/// Retained after Store destruction when pooled; released at checkout when the
/// instance becomes active. No extra heap allocation is needed for each return.
pub(super) struct IdleReservation {
    budget: Arc<GuestBudget>,
    bytes: usize,
    _slot: OwnedSemaphorePermit,
}

impl Drop for IdleReservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub(super) fn try_idle(bytes: usize) -> Option<IdleReservation> {
    global().try_idle(bytes)
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
            return Err("wasm_limits.guest_memory_bytes must be positive and fit the platform allocation range");
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
            budget: global().guest.clone(),
            bytes: AtomicUsize::new(0),
        }
    }
}

impl GuestMemory {
    pub(super) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }

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

/// Independently sampled counters, not a transactional heap/RSS measurement.
#[derive(Serialize, Default)]
pub struct Stats {
    limits: WasmLimits,
    guest_bytes: usize,
    instances: usize,
    preparations: usize,
    idle_instances: usize,
    idle_guest_bytes: usize,
}

pub fn stats() -> Stats {
    // Diagnostics must not initialize defaults before startup configuration.
    ADMISSION.get().map_or_else(Stats::default, Admission::stats)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/admission_test.rs"]
mod tests;
