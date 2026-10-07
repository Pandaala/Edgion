//! Client API owns local AI errors independently of the selected provider.
use crate::ai::AiProxyError;
use crate::plugins::runtime::PluginSession;
use crate::EdgionHttpContext;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::AiApi;
pub(crate) fn render_ai_error(ctx: &EdgionHttpContext, error: AiProxyError) -> (u16, String) {
    if ctx.ai_state().and_then(|s| s.client_api) == Some(AiApi::AnthropicMessages) {
        super::anthropic::error::render(error)
    } else {
        super::openai::error::render_openai_error(error)
    }
}
pub(crate) fn ai_error_response(session: &mut dyn PluginSession, error: AiProxyError) -> PluginRunningResult {
    let (status, body) = render_ai_error(session.ctx(), error);
    let _ = session.set_response_header("content-type", "application/json");
    PluginRunningResult::ErrResponse {
        status,
        body: Some(Bytes::from(body)),
    }
}
