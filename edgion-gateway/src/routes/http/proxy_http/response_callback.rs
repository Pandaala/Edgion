//! Race transport-free response work against canonical downstream closure.

use std::future::{pending, Future};

use pingora_core::{Error, ErrorType, Result};
use pingora_proxy::Session;

fn downstream_error(error: Box<Error>) -> Box<Error> {
    let mut error = error.into_down();
    error.set_retry(false);
    error
}

async fn downstream_closed(session: &mut Session) -> Box<Error> {
    let downstream = session.as_downstream_mut();
    if downstream.is_http2() {
        if let Some(watcher) = downstream.watch_h2_stream_close() {
            return downstream_error(match watcher.await {
                Ok(_) => Error::explain(ErrorType::ConnectionClosed, "downstream reset during response callback"),
                Err(error) => error,
            });
        }
    } else if downstream.as_http1().is_some() && !downstream.was_upgraded() && downstream.is_body_done() {
        // Never idle-read an incomplete upload or replay. This canonical reader
        // owns FIN tolerance and preserves a pipelined probe byte. After that
        // byte is stashed it intentionally stops monitoring this H1 request.
        if let Err(error) = downstream.read_body_or_idle(true).await {
            return downstream_error(error);
        }
    }
    pending().await
}

/// The callback must already own its execution lease when passed here, including
/// before its first poll. Dropping it then releases inflight work even when the
/// close watcher is immediately ready. No task or detached transport loan exists.
pub(super) async fn run<T>(session: &mut Session, callback: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::select! {
        biased;
        error = downstream_closed(session) => Err(error),
        result = callback => result,
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/response_callback_test.rs"]
mod tests;
