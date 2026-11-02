//! Process-owned admission and actual outstanding-output byte credits.
use bytes::Bytes;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, LazyLock,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
const OUTPUT_CAP: usize = 1024 * 1024;
// 64 MiB process budget / reviewed 32 MiB maximum live exchange heap.
static ADMISSION: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));
pub struct Reservation {
    _permit: OwnedSemaphorePermit,
    outstanding: AtomicUsize,
}
impl Reservation {
    pub(super) fn acquire() -> Option<Arc<Self>> {
        Self::acquire_from(&ADMISSION)
    }
    pub(super) fn acquire_from(admission: &Arc<Semaphore>) -> Option<Arc<Self>> {
        admission.clone().try_acquire_owned().ok().map(|permit| {
            Arc::new(Self {
                _permit: permit,
                outstanding: AtomicUsize::new(0),
            })
        })
    }
    pub(super) fn output(self: &Arc<Self>, bytes: Vec<u8>) -> Result<Bytes, ()> {
        let len = bytes.capacity();
        self.outstanding
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(len).filter(|total| *total <= OUTPUT_CAP)
            })
            .map_err(|_| ())?;
        Ok(Bytes::from_owner(OutputOwner {
            bytes,
            reservation: self.clone(),
        }))
    }
}
struct OutputOwner {
    bytes: Vec<u8>,
    reservation: Arc<Reservation>,
}
impl AsRef<[u8]> for OutputOwner {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for OutputOwner {
    fn drop(&mut self) {
        self.reservation
            .outstanding
            .fetch_sub(self.bytes.capacity(), Ordering::AcqRel);
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/grpc_json_transcoder/budget_test.rs"]
mod tests;
