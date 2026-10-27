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
    assert!(
        storage.write(&Bytes::new()).await.is_err(),
        "an empty chunk must not hide poisoned capture state"
    );
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
async fn blocked_snapshot_read_times_out_without_advancing_the_reader() {
    let (mut storage, view) = BodyStorage::with_storage_timeout(1, 64, Duration::from_millis(100));
    storage.write(&Bytes::from_static(b"snapshot-body")).await.unwrap();
    storage.finish().await.unwrap();
    let mut reader = view.snapshot_reader().unwrap();

    let gate = blocking_hook(StorageIoOperation::SnapshotRead);
    let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
        let result = reader.next_chunk().await;
        (reader, result)
    }));
    wait_until_entered(gate.entered).await;

    let (mut reader, result) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("snapshot read returns within its deadline")
        .expect("snapshot reader task joins");
    assert_eq!(result, Err(BodyReadError::StorageFailure));
    gate.release.send(()).expect("release late snapshot read");
    assert_eq!(
        reader.next_chunk().await.unwrap(),
        Some(Bytes::from_static(b"snapshot-body")),
        "a timed-out peek must leave the reader at the same offset"
    );
}

#[tokio::test]
async fn empty_chunk_on_healthy_spill_performs_no_storage_io() {
    let (mut storage, _) = BodyStorage::with_storage_timeout(1, 64, Duration::from_secs(1));
    storage.write(&Bytes::from_static(b"body")).await.unwrap();
    let hook: Arc<dyn Fn(StorageIoOperation) + Send + Sync> = Arc::new(|operation| {
        if operation == StorageIoOperation::SpillWrite {
            panic!("empty capture chunk submitted spill I/O");
        }
    });

    STORAGE_IO_TEST_HOOK
        .scope(hook, storage.write(&Bytes::new()))
        .await
        .expect("empty capture chunk is a no-op");
    assert_eq!(storage.captured_len, 4);
    storage.finish().await.unwrap();
    storage.rewind().await.unwrap();
    assert_eq!(storage.next_chunk(64).await.unwrap(), Some(Bytes::from_static(b"body")));
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

#[tokio::test]
async fn budget_is_retained_by_late_append_after_timeout_or_cancellation() {
    use crate::request_body::budget::{StorageBudget, TEST_BUDGET};
    for cancel in [false, true] {
        let budget = StorageBudget::new(16, 16, 1, 1);
        let (mut storage, view) = TEST_BUDGET
            .scope(Arc::clone(&budget), async {
                let (mut storage, view) = BodyStorage::with_storage_timeout(1, 16, Duration::from_millis(100));
                storage.write(&Bytes::from_static(b"ab")).await.unwrap();
                (storage, view)
            })
            .await;
        let gate = blocking_hook(StorageIoOperation::SpillWrite);
        let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
            let result = storage.write(&Bytes::from_static(b"cd")).await;
            (storage, result)
        }));
        wait_until_entered(gate.entered).await;
        assert_eq!(budget.usage(), [2, 4, 1, 1]);
        if cancel {
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        } else {
            let (storage, result) = task.await.unwrap();
            assert!(result.is_err());
            drop(storage);
        }
        drop(view);
        assert_eq!(budget.usage(), [2, 4, 1, 1]);
        gate.release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while budget.usage() != [0; 4] {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn partially_written_file_keeps_attempted_disk_reservation_until_last_owner() {
    use crate::request_body::budget::StorageBudget;
    let budget = StorageBudget::new(16, 16, 1, 1);
    let lease = budget.capture().unwrap();
    let file = Arc::new(BudgetedFile {
        file: tempfile().unwrap(),
        disk: budget.disk.reserve(0).unwrap().shared(),
        _file_slot: budget.files.reserve(1).unwrap(),
        lease,
    });
    let retained = Arc::clone(&file);
    file.disk.grow(8).unwrap();
    let result: Result<(), StorageIoError> =
        run_storage_io(Duration::from_secs(1), StorageIoOperation::SpillWrite, move || {
            write_all_at(&file, b"part", 0)?;
            Err(io::Error::other("injected failure after a partial append"))
        })
        .await;
    assert!(result.is_err());
    assert_eq!(retained.metadata().unwrap().len(), 4);
    assert_eq!(budget.usage(), [0, 8, 1, 1]);
    drop(retained);
    assert_eq!(budget.usage(), [0; 4]);
}

struct OpaqueInput {
    bytes: Vec<u8>,
    dropped: Arc<AtomicBool>,
}
impl AsRef<[u8]> for OpaqueInput {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for OpaqueInput {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}
fn opaque_input(bytes: &[u8]) -> (Bytes, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    let body = Bytes::from_owner(OpaqueInput {
        bytes: bytes.to_vec(),
        dropped: Arc::clone(&dropped),
    });
    (body, dropped)
}

#[tokio::test]
async fn mutation_releases_opaque_inputs_before_waiting_for_scanner_or_io() {
    use crate::request_body::budget::{StorageBudget, TEST_BUDGET};
    let budget = StorageBudget::new(32, 32, 1, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(1, 32);
            storage.write(&Bytes::from_static(b"abcdef")).await.unwrap();
            storage.finish().await.unwrap();
            let permits = Arc::clone(scan_limiter())
                .acquire_many_owned(SCAN_CONCURRENCY as u32)
                .await
                .unwrap();
            let (from, from_dropped) = opaque_input(b"--ab--");
            let (to, to_dropped) = opaque_input(b"--xy--");
            let mut mutation = Box::pin(view.replace_literal(from.slice(2..4), to.slice(2..4), 32));
            drop(from);
            drop(to);
            assert!(futures::poll!(&mut mutation).is_pending());
            assert!(from_dropped.load(Ordering::Acquire));
            assert!(to_dropped.load(Ordering::Acquire));
            assert_eq!(budget.usage()[0], 2);
            drop(mutation);
            drop(permits);
            assert_eq!(budget.usage()[0], 0);

            let (original, replacement_dropped) = opaque_input(b"--xy--");
            let replacement = original.slice(2..4);
            drop(original);
            let gate = blocking_hook(StorageIoOperation::MaterializeRead);
            let work_view = Arc::clone(&view);
            let task = tokio::spawn(STORAGE_IO_TEST_HOOK.scope(gate.hook, async move {
                work_view.replace_slice(0..2, replacement).await
            }));
            wait_until_entered(gate.entered).await;
            assert!(replacement_dropped.load(Ordering::Acquire));
            assert_eq!(budget.usage()[0], 8);
            gate.release.send(()).unwrap();
            assert_eq!(task.await.unwrap().unwrap(), "xycdef");
            drop(view);
            drop(storage);
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}
