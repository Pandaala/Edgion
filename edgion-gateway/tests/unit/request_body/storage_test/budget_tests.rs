use super::*;
use crate::request_body::budget::{StorageBudget, TEST_BUDGET};
use pingora_core::protocols::http::RequestBodyBuffer;

#[tokio::test]
async fn retained_bytes_and_replay_share_one_capture_and_memory_charge() {
    let budget = StorageBudget::new(8, 32, 1, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(8, 32);
            storage.write(&Bytes::from_static(b"abcd")).await.unwrap();
            storage.finish().await.unwrap();
            let body = view.read_all().await.unwrap();
            let mut reader = view.snapshot_reader().unwrap();
            let replay = reader.next_chunk().await.unwrap().unwrap();
            assert_eq!(budget.usage(), [4, 0, 1, 0]);
            drop(storage);
            drop(view);
            drop(reader);
            drop(body);
            assert_eq!(budget.usage(), [4, 0, 1, 0]);
            assert_eq!(replay, "abcd");
            let (mut refused, _) = BodyStorage::with_limits(8, 32);
            assert!(matches!(
                refused.finish().await.unwrap_err().etype(),
                ErrorType::HTTPStatus(503)
            ));
            drop(replay);
            assert_eq!(budget.usage(), [0; 4]);
            let (mut next, _) = BodyStorage::with_limits(8, 32);
            next.finish().await.unwrap();
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}

#[tokio::test]
async fn spill_and_materialization_are_independently_accounted() {
    let budget = StorageBudget::new(8, 8, 3, 2);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(2, 32);
            storage.write(&Bytes::from_static(b"abcd")).await.unwrap();
            storage.finish().await.unwrap();
            assert_eq!(budget.usage(), [0, 4, 1, 1]);
            let body = view.read_all().await.unwrap();
            let cached = view.read_all().await.unwrap();
            assert_eq!(budget.usage(), [4, 4, 1, 1]);
            assert_eq!(body.as_ptr(), cached.as_ptr());
            let (mut second, second_view) = BodyStorage::with_limits(2, 32);
            second.write(&Bytes::from_static(b"more")).await.unwrap();
            second.finish().await.unwrap();
            let (mut refused, refused_view) = BodyStorage::with_limits(1, 32);
            assert!(matches!(
                refused.write(&Bytes::from_static(b"xx")).await.unwrap_err().etype(),
                ErrorType::HTTPStatus(503)
            ));
            drop(refused);
            drop(refused_view);
            let second_body = second_view.read_all().await.unwrap();
            assert_eq!(budget.usage(), [8, 8, 2, 2]);
            assert_eq!(
                view.read_range(0, 1, 1).await.unwrap_err(),
                BodyReadError::CapacityExceeded.into()
            );
            drop(storage);
            drop(view);
            drop(cached);
            drop(second);
            drop(second_view);
            assert_eq!(budget.usage(), [8, 0, 2, 0]);
            drop(body);
            drop(second_body);
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}

#[tokio::test]
async fn mutation_and_isolation_preserve_old_owners_and_refuse_atomically() {
    let budget = StorageBudget::new(12, 32, 1, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(8, 32);
            storage.write(&Bytes::from_static(b"old!")).await.unwrap();
            storage.finish().await.unwrap();
            let mut old_reader = view.snapshot_reader().unwrap();
            view.overwrite_body(Bytes::from_static(b"new!")).unwrap();
            assert_eq!(budget.usage()[0], 8);
            let mut isolated = view.snapshot_reader().unwrap();
            isolated.isolate_memory().unwrap();
            assert_eq!(budget.usage()[0], 12);
            let generation = view.generation();
            assert_eq!(
                view.append(Bytes::from_static(b"!")).unwrap_err(),
                BodyReadError::CapacityExceeded.into()
            );
            assert_eq!(view.generation(), generation);
            assert_eq!(old_reader.next_chunk().await.unwrap().unwrap(), "old!");
            let isolated_body = isolated.next_chunk().await.unwrap().unwrap();
            assert_eq!(isolated_body, "new!");
            drop(storage);
            drop(view);
            drop(old_reader);
            drop(isolated);
            assert_eq!(budget.usage(), [4, 0, 1, 0]);
            drop(isolated_body);
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}

#[tokio::test]
async fn spill_transition_and_file_ceiling_refuse_before_retaining_resources() {
    let budget = StorageBudget::new(5, 32, 4, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(4, 32);
            storage.write(&Bytes::from_static(b"abcd")).await.unwrap();
            assert!(matches!(
                storage.write(&Bytes::from_static(b"ef")).await.unwrap_err().etype(),
                ErrorType::HTTPStatus(503)
            ));
            assert_eq!(budget.usage(), [0, 0, 1, 0]);
            drop(storage);
            drop(view);
            let (mut disk, disk_view) = BodyStorage::with_limits(1, 32);
            disk.write(&Bytes::from_static(b"ab")).await.unwrap();
            let (mut second, second_view) = BodyStorage::with_limits(1, 32);
            assert!(matches!(
                second.write(&Bytes::from_static(b"cd")).await.unwrap_err().etype(),
                ErrorType::HTTPStatus(503)
            ));
            assert_eq!(budget.usage(), [0, 2, 2, 1]);
            drop(second);
            drop(second_view);
            drop(disk);
            drop(disk_view);
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}

#[tokio::test]
async fn disk_replay_output_retains_memory_and_capture_after_file_closes() {
    let budget = StorageBudget::new(8, 16, 1, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(1, 16);
            storage.write(&Bytes::from_static(b"abcdef")).await.unwrap();
            storage.finish().await.unwrap();
            storage.rewind().await.unwrap();
            let bytes = storage.next_chunk(3).await.unwrap().unwrap();
            assert_eq!(bytes, "abc");
            assert_eq!(budget.usage(), [3, 6, 1, 1]);
            drop(storage);
            drop(view);
            assert_eq!(budget.usage(), [3, 0, 1, 0]);
            drop(bytes);
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}

#[tokio::test]
async fn physical_mutation_limit_precedes_aggregate_admission() {
    let budget = StorageBudget::new(4, 16, 1, 1);
    TEST_BUDGET
        .scope(Arc::clone(&budget), async {
            let (mut storage, view) = BodyStorage::with_limits(4, 4);
            storage.write(&Bytes::from_static(b"body")).await.unwrap();
            storage.finish().await.unwrap();
            let generation = view.generation();
            let error = BodyOperationError::Body(BodyReadError::BufferLimitExceeded { limit: 4 });
            assert_eq!(view.append(Bytes::from_static(b"!")), Err(error.clone()));
            assert_eq!(view.prepend(Bytes::from_static(b"!")), Err(error.clone()));
            assert_eq!(
                view.replace_slice(0..1, Bytes::from_static(b"xx")).await,
                Err(BodyReadError::BufferLimitExceeded { limit: 4 })
            );
            assert_eq!(
                view.replace_spans(generation, &[(0..1, Bytes::from_static(b"xx"))]),
                Err(error)
            );
            assert_eq!(view.generation(), generation);
            assert_eq!(budget.usage(), [4, 0, 1, 0]);
            assert_eq!(view.read_all().await.unwrap(), "body");
        })
        .await;
    assert_eq!(budget.usage(), [0; 4]);
}
