use super::*;
use bytes::Bytes;
use parking_lot::Mutex;
use pingora_core::protocols::http::RequestBodyBuffer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

struct BlockingHook {
    hook: Arc<dyn Fn(StorageIoOperation) + Send + Sync>,
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

fn blocking_hook(target: StorageIoOperation) -> BlockingHook {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let blocked = Arc::new(AtomicBool::new(false));
    let hook = Arc::new(move |operation| {
        if operation != target || blocked.swap(true, Ordering::AcqRel) {
            return;
        }
        entered_tx.send(()).expect("test is waiting for blocked storage I/O");
        release_rx.lock().recv().expect("test releases blocked storage I/O");
    });
    BlockingHook { hook, entered, release }
}

async fn wait_until_entered(entered: mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        entered
            .recv_timeout(Duration::from_secs(2))
            .expect("storage operation enters blocking closure")
    })
    .await
    .expect("entered waiter joins");
}

#[tokio::test]
async fn blocked_spill_creation_times_out_and_capture_stays_poisoned() {
    let gate = blocking_hook(StorageIoOperation::SpillCreate);
    let (mut storage, _) = BodyStorage::with_storage_timeout(1, 64, Duration::from_millis(100));
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        let result = storage.write(&Bytes::from_static(b"spill")).await;
        (storage, result)
    }));

    wait_until_entered(gate.entered).await;
    let (mut storage, result) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("capture returns within its deadline")
        .expect("capture task joins");
    assert!(result.is_err());
    assert!(matches!(storage.storage, CaptureStorage::Poisoned));
    assert_eq!(storage.captured_len, 0);
    assert!(storage.finish().await.is_err());
    gate.release.send(()).expect("release late spill creation");
}

#[tokio::test]
async fn blocked_spill_append_does_not_commit_length_or_restore_storage() {
    let (mut storage, _) = BodyStorage::with_storage_timeout(1, 64, Duration::from_millis(100));
    storage.write(&Bytes::from_static(b"ab")).await.expect("initial spill");
    assert_eq!(storage.captured_len, 2);

    let gate = blocking_hook(StorageIoOperation::SpillWrite);
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        let result = storage.write(&Bytes::from_static(b"cd")).await;
        (storage, result)
    }));
    wait_until_entered(gate.entered).await;

    let (mut storage, result) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("append returns within its deadline")
        .expect("append task joins");
    assert!(result.is_err());
    assert_eq!(storage.captured_len, 2);
    assert!(matches!(storage.storage, CaptureStorage::Poisoned));
    assert!(storage.finish().await.is_err());
    gate.release.send(()).expect("release late append");
}

#[tokio::test]
async fn blocked_replay_read_returns_no_chunk_and_cannot_be_rewound() {
    let (mut storage, _) = BodyStorage::with_storage_timeout(1, 64, Duration::from_millis(100));
    storage.write(&Bytes::from_static(b"replay-body")).await.unwrap();
    storage.finish().await.unwrap();
    storage.rewind().await.unwrap();

    let gate = blocking_hook(StorageIoOperation::ReplayRead);
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        let result = storage.next_chunk(64).await;
        (storage, result)
    }));
    wait_until_entered(gate.entered).await;

    let (mut storage, result) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("replay returns within its deadline")
        .expect("replay task joins");
    assert!(result.is_err(), "a timed-out read must not produce a shortened chunk");
    assert_eq!(storage.replay_cursor, 0);
    assert!(
        storage.rewind().await.is_err(),
        "storage failure is permanent for this request"
    );
    gate.release.send(()).expect("release late replay read");
}

#[tokio::test]
async fn blocked_mutated_materialization_uses_the_storage_deadline() {
    let (mut storage, view) = BodyStorage::with_storage_timeout(1, 64, Duration::from_millis(100));
    storage.write(&Bytes::from_static(b"disk-body")).await.unwrap();
    storage.finish().await.unwrap();
    view.append(Bytes::from_static(b"-suffix")).unwrap();

    let gate = blocking_hook(StorageIoOperation::MaterializeRead);
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move { view.read_all().await }));
    wait_until_entered(gate.entered).await;

    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("materialization returns within its deadline")
        .expect("materialization task joins");
    assert_eq!(result, Err(BodyReadError::StorageFailure));
    gate.release.send(()).expect("release late materialization");
}

#[tokio::test]
async fn slow_storage_within_deadline_remains_usable() {
    let gate = blocking_hook(StorageIoOperation::SpillCreate);
    let (mut storage, _) = BodyStorage::with_storage_timeout(1, 64, Duration::from_secs(1));
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        let result = storage.write(&Bytes::from_static(b"body")).await;
        (storage, result)
    }));
    wait_until_entered(gate.entered).await;
    gate.release.send(()).expect("release spill within deadline");

    let (mut storage, result) = task.await.expect("capture task joins");
    result.expect("slow storage succeeds before deadline");
    storage.finish().await.unwrap();
    storage.rewind().await.unwrap();
    let chunk = storage.next_chunk(64).await.unwrap().expect("replayed chunk");
    assert_eq!(chunk, Bytes::from_static(b"body"));
}

#[tokio::test]
async fn admission_is_bounded_and_permit_outlives_timed_out_waiter() {
    let limiter = Arc::new(Semaphore::new(1));
    let gate = blocking_hook(StorageIoOperation::SpillWrite);
    let first_limiter = Arc::clone(&limiter);
    let first = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        run_storage_io_with_limiter(
            first_limiter,
            Duration::from_millis(150),
            StorageIoOperation::SpillWrite,
            || Ok(()),
        )
        .await
    }));
    wait_until_entered(gate.entered).await;

    let second_entered = Arc::new(AtomicBool::new(false));
    let work_entered = Arc::clone(&second_entered);
    let second = run_storage_io_with_limiter(
        Arc::clone(&limiter),
        Duration::from_millis(50),
        StorageIoOperation::SpillCreate,
        move || {
            work_entered.store(true, Ordering::Release);
            Ok(())
        },
    )
    .await;
    assert!(matches!(second, Err(StorageIoError::AdmissionTimeout)));
    assert!(!second_entered.load(Ordering::Acquire), "unadmitted work was submitted");

    let first = first.await.expect("first waiter returns");
    assert!(matches!(first, Err(StorageIoError::ExecutionTimeout)));
    assert_eq!(limiter.available_permits(), 0, "active syscall still owns its permit");

    gate.release.send(()).expect("release first operation");
    tokio::time::timeout(Duration::from_secs(2), async {
        while limiter.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("permit returns after the real blocking operation ends");
    assert_eq!(limiter.available_permits(), 1);
}
