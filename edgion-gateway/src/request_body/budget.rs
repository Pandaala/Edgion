//! Process-wide admission for storage-owned request payloads, not total heap/RSS.
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use super::BodyReadError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestBodyBudgetConfig {
    pub memory_limit: String,
    pub disk_limit: String,
    pub max_captures: u64,
    pub max_files: u64,
}

impl Default for RequestBodyBudgetConfig {
    fn default() -> Self {
        Self {
            memory_limit: "256MiB".into(),
            disk_limit: "2GiB".into(),
            max_captures: 1024,
            max_files: 1024,
        }
    }
}

impl RequestBodyBudgetConfig {
    fn resolve(&self) -> Result<Arc<StorageBudget>, String> {
        let parse = |name, value: &str| {
            edgion_resources::resources::common::parse_byte_size(value)
                .filter(|bytes| *bytes > 0 && *bytes <= isize::MAX as u64)
                .ok_or_else(|| format!("request_body_budget.{name} must be a positive addressable byte size"))
        };
        let memory = parse("memory_limit", &self.memory_limit)?;
        let disk = parse("disk_limit", &self.disk_limit)?;
        if self.max_captures == 0 || self.max_files == 0 {
            return Err("request_body_budget.max_captures and max_files must be positive".into());
        }
        Ok(StorageBudget::new(memory, disk, self.max_captures, self.max_files))
    }
}

static BUDGET: OnceLock<Arc<StorageBudget>> = OnceLock::new();

pub(crate) fn initialize(config: &RequestBodyBudgetConfig) -> Result<(), String> {
    BUDGET
        .set(config.resolve()?)
        .map_err(|_| "request-body budget already initialized; initialize once before serving".into())
}

#[cfg(not(test))]
pub(crate) use shared_process_budget as process_budget;

pub(crate) fn shared_process_budget() -> Arc<StorageBudget> {
    Arc::clone(BUDGET.get_or_init(|| {
        // Library users get the same finite defaults. CLI startup publishes its
        // validated configuration before serving, and never replaces a budget.
        RequestBodyBudgetConfig::default()
            .resolve()
            .expect("valid built-in body budget")
    }))
}

#[cfg(test)]
include!("../../tests/support/request_body/budget_test/helpers.rs");

pub(crate) struct Counter {
    used: AtomicU64,
    limit: u64,
}

impl Counter {
    fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(0),
            limit,
        })
    }

    pub(crate) fn reserve(self: &Arc<Self>, amount: u64) -> Result<Reservation, BodyReadError> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(amount).filter(|next| *next <= self.limit)
            })
            .map_err(|_| BodyReadError::CapacityExceeded)?;
        Ok(Reservation {
            counter: Arc::clone(self),
            amount,
        })
    }
}

pub(crate) struct Reservation {
    counter: Arc<Counter>,
    amount: u64,
}

impl Reservation {
    pub(crate) fn shared(mut self) -> SharedReservation {
        let shared = SharedReservation {
            counter: Arc::clone(&self.counter),
            amount: AtomicU64::new(self.amount),
        };
        self.amount = 0;
        shared
    }
}

pub(crate) struct SharedReservation {
    counter: Arc<Counter>,
    amount: AtomicU64,
}

impl SharedReservation {
    pub(crate) fn grow(&self, amount: u64) -> Result<(), BodyReadError> {
        let mut extra = self.counter.reserve(amount)?;
        self.amount.fetch_add(extra.amount, Ordering::AcqRel);
        extra.amount = 0;
        Ok(())
    }
}

impl Drop for SharedReservation {
    fn drop(&mut self) {
        self.counter
            .used
            .fetch_sub(self.amount.load(Ordering::Acquire), Ordering::AcqRel);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.counter.used.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

pub(crate) struct StorageBudget {
    pub(crate) memory: Arc<Counter>,
    pub(crate) disk: Arc<Counter>,
    captures: Arc<Counter>,
    pub(crate) files: Arc<Counter>,
}

impl StorageBudget {
    pub(crate) fn new(memory: u64, disk: u64, captures: u64, files: u64) -> Arc<Self> {
        Arc::new(Self {
            memory: Counter::new(memory),
            disk: Counter::new(disk),
            captures: Counter::new(captures),
            files: Counter::new(files),
        })
    }

    pub(crate) fn capture(self: &Arc<Self>) -> Result<Arc<CaptureLease>, BodyReadError> {
        Ok(Arc::new(CaptureLease {
            budget: Arc::clone(self),
            _capture: self.captures.reserve(1)?,
        }))
    }
}

pub(crate) struct CaptureLease {
    pub(crate) budget: Arc<StorageBudget>,
    _capture: Reservation,
}

/// Exact addressable payload capacity. Allocator metadata/rounding is excluded.
/// Fields drop in resource-before-credit order, including exported Bytes owners.
pub(crate) struct MemoryBuffer {
    data: Box<[u8]>,
    len: usize,
    _memory: Reservation,
    lease: Arc<CaptureLease>,
}

impl MemoryBuffer {
    pub(crate) fn new(capacity: usize, lease: Arc<CaptureLease>) -> Result<Self, BodyReadError> {
        let memory = lease.budget.memory.reserve(capacity as u64)?;
        Ok(Self {
            data: vec![0; capacity].into_boxed_slice(),
            len: 0,
            _memory: memory,
            lease,
        })
    }

    pub(crate) fn copy(bytes: &[u8], lease: Arc<CaptureLease>) -> Result<Bytes, BodyReadError> {
        let mut buffer = Self::new(bytes.len(), lease)?;
        buffer.append(bytes);
        Ok(buffer.freeze())
    }

    pub(crate) fn capacity(&self) -> usize {
        self.data.len()
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }
    pub(crate) fn writable(&mut self) -> &mut [u8] {
        &mut self.data
    }
    pub(crate) fn set_len(&mut self, len: usize) {
        self.len = len;
    }
    pub(crate) fn append(&mut self, bytes: &[u8]) {
        let end = self.len + bytes.len();
        self.data[self.len..end].copy_from_slice(bytes);
        self.len = end;
    }
    pub(crate) fn grow(&mut self, capacity: usize) -> Result<(), BodyReadError> {
        let mut next = Self::new(capacity, Arc::clone(&self.lease))?;
        next.append(self.as_slice());
        *self = next;
        Ok(())
    }
    pub(crate) fn freeze(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

impl AsRef<[u8]> for MemoryBuffer {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/budget_test.rs"]
mod tests;
