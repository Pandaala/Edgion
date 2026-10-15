use super::*;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use parking_lot::Mutex;
use std::sync::Arc;

type Finalizations = Arc<Mutex<Vec<(&'static str, &'static str)>>>;

struct TerminalMarker(Finalizations);

#[async_trait::async_trait]
impl StreamingResponseProcessor for TerminalMarker {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        _stream_kind: crate::ctx::ResponseStreamKind,
        _sink: &mut pingora_proxy::ResponseBodySink,
        _session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
    ) {
    }

    fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        self.0.lock().push((outcome, reason));
    }
}

#[tokio::test]
async fn late_semantic_cleanup_preserves_terminal_facts_before_and_after_freeze() {
    use pingora_core::ErrorSource;

    for (source, expected) in [
        (Some(ErrorSource::Upstream), ("upstream-abort", "stream-error")),
        (Some(ErrorSource::Downstream), ("cancelled", "downstream-cancel")),
        (None, ("cancelled", "downstream-cancel")),
        (Some(ErrorSource::Internal), ("fail-close", "internal-stream-error")),
        (Some(ErrorSource::Unset), ("fail-close", "internal-stream-error")),
    ] {
        for frozen in [false, true] {
            let mut ctx = EdgionHttpContext::new();
            let semantic = Finalizations::default();
            let ai = Finalizations::default();
            let ordinary = Finalizations::default();
            let state = ctx.ensure_ai_state();
            state.semantic_processor = Some(Box::new(TerminalMarker(semantic.clone())));
            state.stream_processor = Some(Box::new(TerminalMarker(ai.clone())));
            assert!(ctx.push_response_stream_processor(Box::new(TerminalMarker(ordinary.clone()))));
            if frozen {
                ctx.freeze_response_processor_driver();
            }

            finalize_semantic_for_logging(&mut ctx, source.as_ref()).await;

            assert_eq!(*semantic.lock(), vec![expected]);
            assert!(ai.lock().is_empty());
            assert!(ordinary.lock().is_empty());
        }
    }
}

#[tokio::test]
async fn late_semantic_cleanup_respects_cancelled_driver_lease() {
    let mut ctx = EdgionHttpContext::new();
    let semantic = Finalizations::default();
    ctx.ensure_ai_state().semantic_processor = Some(Box::new(TerminalMarker(semantic.clone())));
    ctx.freeze_response_processor_driver();
    let driver = ctx.response_processor_driver().unwrap();
    drop(driver.body_lease().await.unwrap());

    finalize_semantic_for_logging(&mut ctx, Some(&pingora_core::ErrorSource::Upstream)).await;

    assert!(semantic.lock().is_empty());
}

#[tokio::test]
async fn absent_semantic_cleanup_keeps_ordinary_context_allocation_free() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        // The empty helper future completes without yielding or creating runtime work.
        let mut future = std::pin::pin!(finalize_semantic_for_logging(&mut ctx, None));
        let waker = std::task::Waker::noop();
        let mut task = std::task::Context::from_waker(waker);
        assert!(std::future::Future::poll(future.as_mut(), &mut task).is_ready());
    });
    assert_eq!(allocations.calls, 0);
    assert!(ctx.ai_state().is_none());
    assert!(ctx.response_processor_driver().is_none());
}
