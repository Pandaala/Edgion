//! Native Anthropic Messages API translation.

mod request;
mod response;

pub(crate) use request::{prepare_anthropic_request, AnthropicRequestError};
pub(crate) use response::AnthropicResponseProcessor;

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";
