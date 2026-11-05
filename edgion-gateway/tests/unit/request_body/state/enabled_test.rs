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

struct ReadAllScanner;

impl ReadBodyScanner for ReadAllScanner {
    type Output = std::io::Result<Vec<u8>>;

    fn scan<R: std::io::Read>(self, mut reader: R) -> Self::Output {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).map(|_| output)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn known_empty_pull_scanner_reads_an_empty_body_without_charge() {
    let mut backend = Backend::default();
    backend.mark_known_empty();
    let result = backend.scan_read(ReadAllScanner, 0).await.unwrap();
    assert!(result.value.unwrap().is_empty());
    assert_eq!(result.bytes_scanned, 0);

    let backend = Backend::default();
    assert_eq!(
        backend.scan_read(ReadAllScanner, 0).await.err(),
        Some(BodyOperationError::Body(BodyReadError::NotBuffered))
    );
}

#[tokio::test]
async fn mutation_freeze_survives_empty_state_late_capture_and_view_release() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;

    let mut backend = Backend::default();
    backend.mark_known_empty();
    backend.freeze_mutation();
    for memory_limit in [1, 128] {
        let (mut storage, view) = BodyStorage::new(memory_limit, 1024);
        storage.write(&Bytes::from_static(b"original")).await.unwrap();
        storage.finish().await.unwrap();
        backend.install(Arc::clone(&view));
        assert!(backend.overwrite(Bytes::from_static(b"changed")).is_err());
        assert!(view.overwrite_body(Bytes::from_static(b"changed")).is_err());
        assert_eq!(backend.read_all().await.unwrap().unwrap(), "original");
        backend.release_view();
    }
}
