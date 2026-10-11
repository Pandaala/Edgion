use super::*;
use crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch;
use crate::plugins::runtime::traits::session::{MockPluginSession, PluginSession};
use crate::sse::window::WindowRecord;
use bytes::Bytes;
use parking_lot::Mutex;
use std::sync::Arc;

fn batch(raw: Bytes) -> CanonicalRecordBatch {
    let mut batch = CanonicalRecordBatch::new();
    batch.push(WindowRecord::raw(vec![raw])).unwrap();
    batch
}

#[tokio::test]
async fn feature_mock_preserves_owned_batches_and_short_send_borrows() {
    let resident = Arc::new(Mutex::new(None));
    let publish = Arc::clone(&resident);
    let mut ai = MockAiSessionCapability::new();
    ai.expect_semantic_guard_active().once().return_const(true);
    ai.expect_publish_canonical_records()
        .times(2)
        .returning(move |offered| {
            let mut resident = publish.lock();
            if resident.is_some() {
                Err(offered)
            } else {
                *resident = Some(offered);
                Ok(())
            }
        });
    ai.expect_take_canonical_records()
        .times(2)
        .returning(move || resident.lock().take());
    let mut session = MockPluginSession::new();
    session
        .expect_ai()
        .times(5)
        .return_var(Box::new(ai) as Box<dyn AiSessionCapability>);
    session.expect_get_method().once().return_const("POST".to_owned());
    session
        .expect_read_request_body()
        .once()
        .returning(|_| Ok(Bytes::new()));

    async fn exercise(session: &mut dyn PluginSession) {
        assert!(session.ai().semantic_guard_active());
        let accepted = Bytes::from(vec![b'a'; 3]);
        let accepted_pointer = accepted.as_ptr() as usize;
        assert!(session.ai().publish_canonical_records(batch(accepted)).is_ok());
        assert!(session.read_request_body(None).await.unwrap().is_empty());
        assert_eq!(session.get_method(), "POST");
        let rejected = Bytes::from(vec![b'b'; 4]);
        let rejected_pointer = rejected.as_ptr();
        let returned = session.ai().publish_canonical_records(batch(rejected)).unwrap_err();
        let raw = returned.into_records().pop().unwrap().into_raw_slices();
        assert_eq!(raw[0].as_ptr(), rejected_pointer);
        assert_eq!(raw[0].as_ref(), b"bbbb");
        let raw = session
            .ai()
            .take_canonical_records()
            .unwrap()
            .into_records()
            .pop()
            .unwrap()
            .into_raw_slices();
        assert_eq!(raw[0].as_ptr() as usize, accepted_pointer);
        assert_eq!(raw[0].as_ref(), b"aaa");
        assert!(session.ai().take_canonical_records().is_none());
    }
    fn require_send<T: Send>(_: &T) {}
    let future = exercise(&mut session);
    require_send(&future);
    future.await;
}
