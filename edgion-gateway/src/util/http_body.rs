//! Bounded body reception for separate Gateway outbound exchanges.

use bytes::Bytes;
use futures::{Stream, StreamExt};

/// Stream a response body, accumulating at most `max_bytes`.
///
/// Returns collected bytes and whether the body exceeded the cap. Logical
/// accumulation is bounded by `max_bytes`; allocation capacity and a delivered
/// transport chunk also contribute to peak memory.
pub(crate) async fn stream_body_capped(
    resp: reqwest::Response,
    max_bytes: usize,
) -> Result<(Bytes, bool), reqwest::Error> {
    stream_body_chunks_capped(resp.bytes_stream(), max_bytes).await
}

pub(crate) async fn stream_body_chunks_capped<S, E>(stream: S, max_bytes: usize) -> Result<(Bytes, bool), E>
where
    S: Stream<Item = Result<Bytes, E>>,
{
    futures::pin_mut!(stream);
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        // Empty, failed and still-pending bodies need no accumulation storage.
        if buf.capacity() == 0 && !chunk.is_empty() {
            buf.reserve_exact(8192.min(max_bytes));
        }
        let remaining = max_bytes - buf.len();
        if chunk.len() > remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            return Ok((Bytes::from(buf), true));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((Bytes::from(buf), false))
}

#[cfg(test)]
#[path = "../../tests/unit/util/http_body_test.rs"]
mod tests;
