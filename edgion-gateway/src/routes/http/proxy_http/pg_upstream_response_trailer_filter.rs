use http::HeaderMap;
use pingora_proxy::Session;

use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::{PluginLog, UpstreamTrailerResult};
use crate::EdgionHttpContext;

/// Dispatch the request-owned trailer companion vector serially.
///
/// `trailers=None` is a real no-trailers terminal observation, never a
/// fabricated empty map. On fail-close, later companions are synchronously
/// preempted and the proxy returns before the transport can forward trailers.
pub(crate) async fn dispatch_upstream_response_trailers(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    mut trailers: Option<&mut HeaderMap>,
) -> pingora_core::Result<()> {
    if !ctx.response_processors_frozen() {
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "response trailer callback arrived before processor freeze",
        ));
    }
    let claimed = ctx.claim_upstream_response_trailers(trailers.is_some());
    if !claimed {
        return Ok(());
    }

    let Some(driver) = ctx.response_processor_driver() else {
        return Ok(());
    };
    let Some(mut lease) = driver.trailer_lease().await else {
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "response trailer processor group is closed after callback cancellation",
        ));
    };
    if lease.companions().is_empty() {
        lease.complete();
        return Ok(());
    }

    let mut logs = Vec::with_capacity(lease.companions().len());
    let mut fail_close_at = None;
    {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        for (index, companion) in lease.companions().iter_mut().enumerate() {
            let mut log = PluginLog::with_alias(companion.name(), companion.alias());
            let outcome = companion
                .run_upstream_response_trailer(trailers.as_deref_mut(), &mut adapter, &mut log)
                .await;
            logs.push(log);
            if matches!(outcome, UpstreamTrailerResult::FailClose) {
                fail_close_at = Some(index);
                break;
            }
        }
    }

    if let Some(index) = fail_close_at {
        for companion in lease.companions().iter_mut().skip(index + 1) {
            companion.on_terminal_preempted("earlier-trailer-fail-close");
        }
        ctx.finish_plugin_stage("upstream_response_trailers", logs);
        lease.complete();
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "upstream response trailer processor failed closed",
        ));
    }

    ctx.finish_plugin_stage("upstream_response_trailers", logs);
    lease.complete();
    Ok(())
}

pub(crate) async fn finalize_incomplete_trailer_companions(ctx: &mut EdgionHttpContext) {
    if !ctx.claim_upstream_trailer_cleanup() {
        return;
    }
    if let Some(driver) = ctx.response_processor_driver() {
        driver.finalize_trailers_incomplete().await;
        return;
    }
    if let Some(companions) = ctx.upstream_response_trailers_mut() {
        for companion in companions {
            companion.finalize_incomplete("logging-fallback");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::UpstreamResponseTrailer;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;

    async fn dispatch_upstream_response_trailers(
        session: &mut Session,
        ctx: &mut EdgionHttpContext,
        trailers: Option<&mut HeaderMap>,
    ) -> pingora_core::Result<()> {
        ctx.freeze_response_processor_driver();
        super::dispatch_upstream_response_trailers(session, ctx, trailers).await
    }

    async fn test_session() -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"GET /trailers HTTP/1.1\r\nHost: test\r\n\r\n")
            .await
            .expect("write request");
        session.read_request().await.expect("read request");
        session
    }

    struct RecordingCompanion {
        id: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
        calls: Arc<AtomicUsize>,
        fail_close: bool,
        preempted: Arc<AtomicBool>,
    }

    #[async_trait]
    impl UpstreamResponseTrailer for RecordingCompanion {
        fn name(&self) -> &str {
            self.id
        }

        async fn run_upstream_response_trailer(
            &mut self,
            trailers: Option<&mut HeaderMap>,
            _session: &mut dyn crate::plugins::runtime::PluginSession,
            _log: &mut PluginLog,
        ) -> UpstreamTrailerResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.order.lock().expect("order lock").push(self.id);
            if let Some(trailers) = trailers {
                trailers.append("x-order", http::HeaderValue::from_static(self.id));
            }
            if self.fail_close {
                UpstreamTrailerResult::FailClose
            } else {
                UpstreamTrailerResult::Continue
            }
        }

        fn on_terminal_preempted(&mut self, _reason: &'static str) {
            self.preempted.store(true, Ordering::SeqCst);
        }
    }

    fn recording(
        id: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
        calls: Arc<AtomicUsize>,
        fail_close: bool,
        preempted: Arc<AtomicBool>,
    ) -> Box<dyn UpstreamResponseTrailer> {
        Box::new(RecordingCompanion {
            id,
            order,
            calls,
            fail_close,
            preempted,
        })
    }

    #[tokio::test]
    async fn real_trailers_run_serially_and_only_once() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let order = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        for id in ["route", "backend", "gateway", "global"] {
            ctx.install_upstream_response_trailer(recording(
                id,
                Arc::clone(&order),
                Arc::clone(&calls),
                false,
                Arc::new(AtomicBool::new(false)),
            ));
        }
        assert!(ctx.mark_upstream_before_trailers());
        let mut trailers = HeaderMap::new();

        dispatch_upstream_response_trailers(&mut session, &mut ctx, Some(&mut trailers))
            .await
            .expect("first trailer dispatch");
        dispatch_upstream_response_trailers(&mut session, &mut ctx, Some(&mut trailers))
            .await
            .expect("duplicate transport evidence is ignored");

        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(
            *order.lock().expect("order lock"),
            ["route", "backend", "gateway", "global"]
        );
        assert_eq!(
            trailers
                .get_all("x-order")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["route", "backend", "gateway", "global"]
        );
    }

    #[tokio::test]
    async fn no_trailers_is_distinct_and_exactly_once() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let calls = Arc::new(AtomicUsize::new(0));
        ctx.install_upstream_response_trailer(recording(
            "observer",
            Arc::new(Mutex::new(Vec::new())),
            Arc::clone(&calls),
            false,
            Arc::new(AtomicBool::new(false)),
        ));

        dispatch_upstream_response_trailers(&mut session, &mut ctx, None)
            .await
            .unwrap();
        dispatch_upstream_response_trailers(&mut session, &mut ctx, None)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn trailer_event_before_final_header_freeze_is_an_invariant_error() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();

        let error = super::dispatch_upstream_response_trailers(&mut session, &mut ctx, None)
            .await
            .expect_err("unfrozen trailer callback must fail closed");

        assert!(matches!(error.etype(), pingora_core::ErrorType::InternalError));
        assert!(!ctx.response_processors_frozen());
    }

    #[tokio::test]
    async fn fail_close_stops_chain_and_preempts_remaining_companions() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let order = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let last_preempted = Arc::new(AtomicBool::new(false));
        ctx.install_upstream_response_trailer(recording(
            "first",
            Arc::clone(&order),
            Arc::clone(&calls),
            false,
            Arc::new(AtomicBool::new(false)),
        ));
        ctx.install_upstream_response_trailer(recording(
            "failure",
            Arc::clone(&order),
            Arc::clone(&calls),
            true,
            Arc::new(AtomicBool::new(false)),
        ));
        ctx.install_upstream_response_trailer(recording(
            "last",
            Arc::clone(&order),
            Arc::clone(&calls),
            false,
            Arc::clone(&last_preempted),
        ));
        let mut trailers = HeaderMap::new();

        assert!(
            dispatch_upstream_response_trailers(&mut session, &mut ctx, Some(&mut trailers))
                .await
                .is_err()
        );
        assert_eq!(*order.lock().expect("order lock"), ["first", "failure"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(last_preempted.load(Ordering::SeqCst));
    }

    struct PendingCompanion {
        released: Arc<AtomicBool>,
        finalized: Option<Arc<AtomicUsize>>,
    }

    #[async_trait]
    impl UpstreamResponseTrailer for PendingCompanion {
        fn name(&self) -> &str {
            "pending"
        }

        async fn run_upstream_response_trailer(
            &mut self,
            _trailers: Option<&mut HeaderMap>,
            _session: &mut dyn crate::plugins::runtime::PluginSession,
            _log: &mut PluginLog,
        ) -> UpstreamTrailerResult {
            std::future::pending().await
        }

        fn release_inflight(&mut self) {
            self.released.store(true, Ordering::SeqCst);
        }

        fn finalize_incomplete(&mut self, _reason: &'static str) {
            if let Some(finalized) = &self.finalized {
                finalized.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_releases_inflight_companion() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let released = Arc::new(AtomicBool::new(false));
        assert!(ctx.install_upstream_response_trailer(Box::new(PendingCompanion {
            released: Arc::clone(&released),
            finalized: None,
        })));
        let mut trailers = HeaderMap::new();
        let mut future = Box::pin(dispatch_upstream_response_trailers(
            &mut session,
            &mut ctx,
            Some(&mut trailers),
        ));

        assert!(tokio::time::timeout(std::time::Duration::from_millis(10), &mut future)
            .await
            .is_err());
        drop(future);
        assert!(released.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn logging_fallback_finalizes_once() {
        let mut ctx = EdgionHttpContext::default();
        let finalized = Arc::new(AtomicUsize::new(0));
        assert!(ctx.install_upstream_response_trailer(Box::new(PendingCompanion {
            released: Arc::new(AtomicBool::new(false)),
            finalized: Some(Arc::clone(&finalized)),
        })));

        finalize_incomplete_trailer_companions(&mut ctx).await;
        finalize_incomplete_trailer_companions(&mut ctx).await;

        assert_eq!(finalized.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_rejects_installation_after_pre_trailer_boundary() {
        use crate::plugins::runtime::PluginSession;

        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.reserve_response_framing();
        assert!(ctx.mark_upstream_before_trailers());
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

        assert!(!adapter.install_upstream_response_trailer(Box::new(PendingCompanion {
            released: Arc::new(AtomicBool::new(false)),
            finalized: None,
        })));
        assert_eq!(adapter.upstream_response_trailer_count(), 0);
    }

    #[tokio::test]
    async fn adapter_rejects_installation_after_body_processing_starts() {
        use crate::plugins::runtime::PluginSession;

        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.response_body_processing_started = true;
        assert!(ctx.upstream_trailer_lifecycle_is_open());
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

        assert!(!adapter.install_upstream_response_trailer(Box::new(PendingCompanion {
            released: Arc::new(AtomicBool::new(false)),
            finalized: None,
        })));
        assert_eq!(adapter.upstream_response_trailer_count(), 0);
    }

    #[test]
    fn lifecycle_keeps_pre_trailer_boundary_open_for_real_trailers() {
        let mut lifecycle = crate::ctx::UpstreamTrailerLifecycle::Open;
        assert!(lifecycle.mark_before_trailers());
        assert!(!lifecycle.claim_without_trailers());
        assert!(lifecycle.claim_real_trailers());
        assert!(!lifecycle.claim_real_trailers());
    }
}
