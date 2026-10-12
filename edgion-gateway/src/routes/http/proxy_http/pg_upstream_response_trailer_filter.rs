use http::HeaderMap;
use pingora_proxy::Session;

use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::{PluginLog, UpstreamTrailerResult};
use crate::EdgionHttpContext;

#[inline]
fn trailer_processing_error() -> Box<pingora_core::Error> {
    let mut error = pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(500),
        "edgion response trailer processing failed",
    );
    error.set_retry(false);
    error
}

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
    if trailers.is_some() {
        crate::response_cache::lifecycle::abandon(ctx);
    }
    if !ctx.response_processors_frozen() {
        return Err(trailer_processing_error());
    }
    let claimed = ctx.claim_upstream_response_trailers(trailers.is_some());
    if !claimed {
        return Ok(());
    }
    let grpc_evidence = ctx.grpc_transcoding.as_ref().and_then(|_| {
        trailers
            .as_deref()
            .map(crate::plugins::http::grpc_json_transcoder::status_evidence)
    });

    let Some(driver) = ctx.response_processor_driver() else {
        return Ok(());
    };
    let Some(mut lease) = driver.trailer_lease().await else {
        return Err(trailer_processing_error());
    };
    if lease.companions().is_empty() {
        lease.complete();
        if ctx.grpc_transcoding.is_some() {
            driver
                .finalize_grpc_transcoding(
                    grpc_evidence.as_ref(),
                    ctx.upstream_response_headers_end_stream == Some(true),
                )
                .await?;
        }
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
        return Err(trailer_processing_error());
    }

    ctx.finish_plugin_stage("upstream_response_trailers", logs);
    lease.complete();
    if ctx.grpc_transcoding.is_some() {
        driver
            .finalize_grpc_transcoding(
                grpc_evidence.as_ref(),
                ctx.upstream_response_headers_end_stream == Some(true),
            )
            .await?;
    }
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
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_response_trailer_filter_test.rs"]
mod tests;
