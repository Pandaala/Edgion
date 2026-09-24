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
