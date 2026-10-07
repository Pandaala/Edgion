use super::*;
use pingora_proxy::RequestBodyEvent;

struct NoopStreamHandler;

#[async_trait::async_trait]
impl crate::plugins::runtime::traits::RequestBodyStreamHandler for NoopStreamHandler {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _event: RequestBodyEvent,
    ) -> crate::plugins::runtime::traits::ChunkVerdict {
        crate::plugins::runtime::traits::ChunkVerdict::Continue
    }

    fn take_log(&mut self) -> Option<crate::plugins::runtime::log::PluginLog> {
        None
    }

    fn dye_plan(&self) -> Option<Arc<crate::plugins::runtime::dye_plan::CompiledDyePlan>> {
        None
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn request_body_plugin_state_has_one_small_outer_box() {
    assert_eq!(std::mem::size_of::<RequestBodyPluginState>(), 40);
    assert_eq!(std::mem::size_of::<Option<Box<RequestBodyPluginState>>>(), 8);
}

#[test]
fn first_waf_stream_activation_allocates_one_state_and_is_monotonic() {
    let mut ctx = EdgionHttpContext::new();
    let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.activate_waf_request_body_streaming();
    });
    assert!(ctx.has_request_body_plugins());
    assert!(ctx.request_body_stream_active());
    assert_eq!(first.calls, 1);
    assert_eq!(first.requested_bytes, std::mem::size_of::<RequestBodyPluginState>());

    let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.activate_waf_request_body_streaming();
    });
    assert_eq!(later.calls, 0);
    assert!(ctx.request_body_stream_active());
}

#[test]
fn observer_source_does_not_disable_request_replay() {
    let mut ctx = EdgionHttpContext::new();
    let observers = Box::new(crate::request_body::observer::RequestBodyObservers::default());
    let (_, allocation) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.install_request_body_observers(observers);
    });

    assert!(ctx.has_request_body_plugins());
    assert!(ctx.has_request_body_observers());
    assert!(!ctx.request_body_stream_active());
    assert_eq!(allocation.calls, 1);
    assert_eq!(
        allocation.requested_bytes,
        std::mem::size_of::<RequestBodyPluginState>()
    );
}

#[test]
fn retiring_mirror_only_state_restores_outer_fast_path() {
    let mut ctx = EdgionHttpContext::new();
    ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
    ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

    ctx.retire_request_mirror();

    assert!(!ctx.has_mirror_state());
    assert!(!ctx.has_request_body_plugins());
}

#[test]
fn retiring_mirror_preserves_handler_and_waf_replay_latches() {
    let mut handler_ctx = EdgionHttpContext::new();
    assert!(handler_ctx.push_request_body_stream_handler(Box::new(NoopStreamHandler)));
    handler_ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
    handler_ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

    handler_ctx.retire_request_mirror();

    assert!(!handler_ctx.has_mirror_state());
    assert!(handler_ctx.has_request_body_plugins());
    assert_eq!(handler_ctx.request_body_stream_handler_count(), 1);
    assert!(handler_ctx.request_body_stream_active());

    let mut waf_ctx = EdgionHttpContext::new();
    waf_ctx.activate_waf_request_body_streaming();
    waf_ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
    waf_ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

    waf_ctx.retire_request_mirror();

    assert!(!waf_ctx.has_mirror_state());
    assert!(waf_ctx.has_request_body_plugins());
    assert!(waf_ctx.request_body_stream_active());
}

#[test]
fn retiring_mirror_preserves_observer_state_without_disabling_replay() {
    let mut ctx = EdgionHttpContext::new();
    ctx.install_request_body_observers(Box::new(crate::request_body::observer::RequestBodyObservers::default()));
    ctx.activate_request_body_source(REQUEST_BODY_SOURCE_MIRROR);
    ctx.ensure_plugin_aux().mirror_state = Some(MirrorState::Abandoned);

    ctx.retire_request_mirror();

    assert!(!ctx.has_mirror_state());
    assert!(ctx.has_request_body_plugins());
    assert!(ctx.has_request_body_observers());
    assert!(!ctx.request_body_stream_active());
}
