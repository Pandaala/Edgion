//! Existing provider endpoint and credential preparation.

use crate::ai::runtime::dispatch::AiDispatchError;
use crate::ai::RuntimeAiEndpoint;
use crate::plugins::runtime::traits::PluginSession;
use edgion_resources::resources::edgion_backend::AiProvider;

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub(crate) fn prepare_provider_request(
    adapter: &mut impl PluginSession,
    provider: AiProvider,
    endpoint: &RuntimeAiEndpoint,
    credential: &str,
    canonical_path: &str,
    canonical_query: Option<&str>,
) -> Result<(), AiDispatchError> {
    let uri = provider_uri(provider, &endpoint.path_prefix, canonical_path, canonical_query);
    for header in [
        "authorization",
        "x-api-key",
        "api-key",
        "anthropic-version",
        "anthropic-beta",
        "accept-encoding",
    ] {
        adapter
            .remove_request_header(header)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    }
    adapter
        .set_upstream_uri(&uri)
        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    adapter
        .set_upstream_host(&endpoint.host)
        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    match provider {
        AiProvider::OpenAI | AiProvider::OpenAICompatible => {
            let authorization = format!("Bearer {}", credential);
            adapter
                .set_request_header("authorization", &authorization)
                .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        }
        AiProvider::Anthropic => {
            for (name, value) in [
                ("x-api-key", credential),
                ("anthropic-version", ANTHROPIC_VERSION),
                ("content-type", "application/json"),
            ] {
                adapter
                    .set_request_header(name, value)
                    .map_err(|_| AiDispatchError::InvalidConfiguration)?;
            }
        }
    }
    Ok(())
}

fn provider_uri(provider: AiProvider, prefix: &str, canonical_path: &str, canonical_query: Option<&str>) -> String {
    let downstream_path = match provider {
        AiProvider::Anthropic => "/v1/messages",
        AiProvider::OpenAI | AiProvider::OpenAICompatible => canonical_path,
    };
    let prefix = prefix.trim_end_matches('/');
    let path = downstream_path.trim_start_matches('/');
    let mut result = if prefix.is_empty() {
        format!("/{path}")
    } else {
        format!("{prefix}/{path}")
    };
    if provider != AiProvider::Anthropic {
        if let Some(query) = canonical_query {
            result.push('?');
            result.push_str(query);
        }
    }
    result
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/connectors/provider_test.rs"]
mod tests;
