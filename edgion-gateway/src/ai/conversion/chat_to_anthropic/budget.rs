//! Nonblocking admission and shared exchange/output ownership.
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
pub(crate) struct UnaryBudget {
    semaphore: Arc<Semaphore>,
}
pub(crate) struct Reservation {
    _permit: OwnedSemaphorePermit,
}
impl UnaryBudget {
    pub(crate) fn new(slots: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(slots)),
        }
    }
    pub(crate) fn acquire(&self) -> Option<Arc<Reservation>> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| Arc::new(Reservation { _permit: permit }))
    }
}
struct OutputOwner {
    bytes: Vec<u8>,
    _reservation: Arc<Reservation>,
}
impl AsRef<[u8]> for OutputOwner {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
pub(crate) fn output(bytes: Vec<u8>, reservation: Arc<Reservation>) -> Bytes {
    Bytes::from_owner(OutputOwner {
        bytes,
        _reservation: reservation,
    })
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/conversion/chat_to_anthropic/budget_test.rs"]
mod tests;
