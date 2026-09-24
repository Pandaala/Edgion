use super::*;
use crate::request_body::ScanControl;

struct BlockingFinishScanner {
    runtime_thread: std::thread::ThreadId,
}

impl RawBodyScanner for BlockingFinishScanner {
    type Output = bool;

    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, _: &[u8]) -> Result<ScanControl, BodyOperationError> {
        unreachable!("a known-empty body must not push bytes")
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(std::thread::current().id() != self.runtime_thread)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn known_empty_blocking_scanner_finishes_off_the_runtime_thread() {
    let mut backend = Backend::default();
    backend.mark_known_empty();
    let result = backend
        .scan_raw(
            BlockingFinishScanner {
                runtime_thread: std::thread::current().id(),
            },
            0,
        )
        .await
        .unwrap();

    assert!(result.value);
    assert_eq!(result.bytes_scanned, 0);
}
