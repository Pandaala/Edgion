use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct BodyMarker(Arc<AtomicUsize>);

#[async_trait]
impl StreamingResponseProcessor for BodyMarker {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        _stream_kind: crate::ctx::ResponseStreamKind,
        _sink: &mut pingora_proxy::ResponseBodySink,
        _session: &mut dyn crate::plugins::runtime::PluginSession,
    ) {
    }

    fn release_inflight(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct TrailerMarker(Arc<AtomicUsize>);

#[async_trait]
impl UpstreamResponseTrailer for TrailerMarker {
    fn name(&self) -> &str {
        "trailer-marker"
    }

    async fn run_upstream_response_trailer(
        &mut self,
        _trailers: Option<&mut http::HeaderMap>,
        _session: &mut dyn crate::plugins::runtime::PluginSession,
        _log: &mut crate::plugins::runtime::PluginLog,
    ) -> crate::plugins::runtime::UpstreamTrailerResult {
        crate::plugins::runtime::UpstreamTrailerResult::Continue
    }

    fn release_inflight(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn driver(body: Arc<AtomicUsize>, trailer: Arc<AtomicUsize>) -> Arc<ResponseProcessorDriver> {
    ResponseProcessorDriver::new(
        Some(Box::new(BodyMarker(body))),
        None,
        Vec::new(),
        vec![Box::new(TrailerMarker(trailer))],
    )
    .expect("non-empty driver")
}

#[tokio::test]
async fn body_cancellation_closes_only_the_body_group() {
    let body = Arc::new(AtomicUsize::new(0));
    let trailer = Arc::new(AtomicUsize::new(0));
    let driver = driver(Arc::clone(&body), Arc::clone(&trailer));

    drop(driver.body_lease().await.expect("first body lease"));

    assert_eq!(body.load(Ordering::SeqCst), 1);
    assert_eq!(trailer.load(Ordering::SeqCst), 0);
    assert!(driver.body_lease().await.is_none());
    driver.trailer_lease().await.expect("trailer remains open").complete();
    assert_eq!(trailer.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn trailer_cancellation_closes_only_the_trailer_group() {
    let body = Arc::new(AtomicUsize::new(0));
    let trailer = Arc::new(AtomicUsize::new(0));
    let driver = driver(Arc::clone(&body), Arc::clone(&trailer));

    driver.body_lease().await.expect("body lease").complete();
    drop(driver.trailer_lease().await.expect("first trailer lease"));

    assert_eq!(body.load(Ordering::SeqCst), 0);
    assert_eq!(trailer.load(Ordering::SeqCst), 1);
    assert!(driver.trailer_lease().await.is_none());
    driver.body_lease().await.expect("body remains open").complete();
    assert_eq!(body.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelled_unary_callback_releases_its_branch_promptly() {
    use crate::ai::conversion::chat_to_anthropic::{
        budget::UnaryBudget, head::UnaryHead, limits::UnaryLimits, AnthropicResponseProcessor,
    };
    use crate::plugins::runtime::traits::session::MockPluginSession;
    struct PendingUnary {
        inner: AnthropicResponseProcessor,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
    }
    #[async_trait]
    impl StreamingResponseProcessor for PendingUnary {
        async fn on_chunk(
            &mut self,
            chunk: &mut Option<bytes::Bytes>,
            eos: bool,
            kind: crate::ctx::ResponseStreamKind,
            sink: &mut pingora_proxy::ResponseBodySink,
            session: &mut dyn crate::plugins::runtime::PluginSession,
        ) {
            self.inner.on_chunk(chunk, eos, kind, sink, session).await;
            let _ = self.entered.take().unwrap().send(());
            std::future::pending::<()>().await;
        }
        fn release_inflight(&mut self) {
            self.inner.release_inflight();
        }
        fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
            self.inner.finalize_incomplete(outcome, reason);
        }
    }
    let pool = UnaryBudget::new(1);
    let limits = UnaryLimits::resolve(&crate::ai::config::UnaryConfig::default()).unwrap();
    let owner = Arc::new(UnaryHead::new(limits, pool.acquire().unwrap()));
    let usage = Arc::new(crate::ai::AiUsageShared::default());
    let mut processor = AnthropicResponseProcessor::new_with_quota_and_unary(
        "canonical".into(),
        false,
        false,
        1,
        usage.clone(),
        None,
        false,
        Some(owner.clone()),
    );
    let mut head = pingora_http::ResponseHeader::build(200, None).unwrap();
    head.insert_header("content-type", "application/json").unwrap();
    processor.on_response_header(&mut head);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let driver = ResponseProcessorDriver::new(
        Some(Box::new(PendingUnary {
            inner: processor,
            entered: Some(tx),
        })),
        None,
        vec![],
        vec![],
    )
    .unwrap();
    let task_driver = driver.clone();
    let callback = tokio::spawn(async move {
        let mut lease = task_driver.body_lease().await.unwrap();
        let mut chunk = Some(bytes::Bytes::from_static(b"{\"partial\":"));
        let mut sink = pingora_proxy::ResponseBodySink::new();
        let mut session = MockPluginSession::new();
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        ai.expect_semantic_guard_active().return_const(false);
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
        lease
            .processors()
            .ai
            .as_mut()
            .unwrap()
            .on_chunk(
                &mut chunk,
                false,
                crate::ctx::ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;
        lease.complete();
    });
    rx.await.unwrap();
    assert!(pool.acquire().is_none());
    callback.abort();
    assert!(callback.await.unwrap_err().is_cancelled());
    assert!(driver.body_lease().await.is_none());
    assert!(
        pool.acquire().is_none(),
        "the exchange branch still owns the reservation"
    );
    drop(owner);
    assert!(
        pool.acquire().is_some(),
        "callback cancellation must release its owner without a timer or accounting IO"
    );
    assert!(usage.observation_unavailable());
    driver.finalize_ai_incomplete().await;
}
