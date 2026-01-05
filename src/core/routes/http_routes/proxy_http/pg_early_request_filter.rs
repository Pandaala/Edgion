use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_proxy::Session;

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    _ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Set client timeouts from pre-parsed config (no runtime overhead)
    let client_timeout = &edgion_http.parsed_timeouts.client;
    session.set_read_timeout(Some(client_timeout.read_timeout));
    session.set_write_timeout(Some(client_timeout.write_timeout));
    session.set_keepalive(Some(client_timeout.keepalive_timeout));

    Ok(())
}
