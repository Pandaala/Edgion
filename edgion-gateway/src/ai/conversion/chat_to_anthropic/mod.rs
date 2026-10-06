//! OpenAI Chat Completions conversion to and from Anthropic Messages.

mod request;
mod response;

pub(crate) use request::{prepare_anthropic_request, AnthropicRequestError};
pub(crate) use response::AnthropicResponseProcessor;
