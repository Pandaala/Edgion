//! Existing provider endpoint and credential preparation.

use crate::ai::runtime::dispatch::AiDispatchError;
use crate::ai::AiProxyError;
use crate::ai::RuntimeAiEndpoint;
use crate::plugins::runtime::traits::PluginSession;
use edgion_resources::resources::edgion_backend::AiProvider;
use edgion_resources::resources::edgion_plugins::AiApi;

const ANTHROPIC_VERSION: &str = "2023-06-01";

// The canonical boundary keeps protocol, credential and transport inputs explicit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_provider_request(
    adapter: &mut impl PluginSession,
    provider: AiProvider,
    api: AiApi,
    endpoint: &RuntimeAiEndpoint,
    credential: &str,
    canonical_path: &str,
    canonical_query: Option<&str>,
    require_identity: bool,
) -> Result<bool, AiDispatchError> {
    let native = api == AiApi::AnthropicMessages;
    let supported_version = if native {
        validate_native_headers(adapter)?
    } else {
        true
    };
    let uri = provider_uri(provider, &endpoint.path_prefix, canonical_path, canonical_query, native);
    for header in [
        "authorization",
        "x-api-key",
        "api-key",
        "anthropic-version",
        "anthropic-beta",
        "openai-organization",
        "openai-project",
    ] {
        if native && matches!(header, "anthropic-version" | "anthropic-beta") {
            continue;
        }
        adapter
            .remove_request_header(header)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    }
    if native {
        adapter
            .remove_request_header("anthropic-workspace-id")
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    }
    if require_identity {
        adapter
            .remove_request_header("accept-encoding")
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        adapter
            .set_request_header_value("accept-encoding", http::HeaderValue::from_static("identity"))
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
            adapter
                .set_request_header("x-api-key", credential)
                .map_err(|_| AiDispatchError::InvalidConfiguration)?;
            for (name, value) in [
                ("anthropic-version", ANTHROPIC_VERSION),
                ("content-type", "application/json"),
            ] {
                if native && name == "anthropic-version" && native_has_version(adapter) {
                    continue;
                }
                adapter
                    .set_request_header_value(name, http::HeaderValue::from_static(value))
                    .map_err(|_| AiDispatchError::InvalidConfiguration)?;
            }
        }
    }
    Ok(supported_version)
}

fn native_has_version(adapter: &impl PluginSession) -> bool {
    adapter.request_header_map().contains_key("anthropic-version")
        || adapter
            .ctx()
            .deferred_credential_mutations()
            .is_some_and(|m| m.contains_target("anthropic-version"))
}

fn validate_native_headers(adapter: &impl PluginSession) -> Result<bool, AiDispatchError> {
    if [
        "authorization",
        "x-api-key",
        "api-key",
        "anthropic-workspace-id",
        "anthropic-user-profile-id",
        "anthropic-version",
        "anthropic-beta",
    ]
    .iter()
    .any(|h| adapter.is_remote_ip_header(h))
    {
        return Err(AiDispatchError::Client(AiProxyError::UnsupportedParameter));
    }
    let deferred = adapter.ctx().deferred_credential_mutations();
    if adapter.request_header_map().contains_key("anthropic-user-profile-id")
        || deferred.is_some_and(|m| m.contains_target("anthropic-user-profile-id"))
    {
        return Err(AiDispatchError::Client(AiProxyError::UnsupportedParameter));
    }
    if let Some(version) = deferred.and_then(|m| m.value_for("anthropic-version")) {
        return Ok(version.as_bytes() == ANTHROPIC_VERSION.as_bytes());
    }
    Ok(!adapter.request_header_map().contains_key("anthropic-version")
        || version_supported(adapter.request_header_map()))
}

fn version_supported(headers: &http::HeaderMap) -> bool {
    let mut values = headers.get_all("anthropic-version").iter();
    values
        .next()
        .is_some_and(|v| v.as_bytes() == ANTHROPIC_VERSION.as_bytes())
        && values.next().is_none()
}

fn provider_uri(
    provider: AiProvider,
    prefix: &str,
    canonical_path: &str,
    canonical_query: Option<&str>,
    native: bool,
) -> String {
    let downstream_path = match provider {
        AiProvider::Anthropic => "/v1/messages",
        AiProvider::OpenAI | AiProvider::OpenAICompatible => canonical_path,
    };
    let prefix = prefix.trim_end_matches('/');
    let path = downstream_path.trim_start_matches('/');
    let query = canonical_query.filter(|_| native || provider != AiProvider::Anthropic);
    let mut result = String::with_capacity(prefix.len() + 1 + path.len() + query.map_or(0, |value| 1 + value.len()));
    result.push_str(prefix);
    result.push('/');
    result.push_str(path);
    if let Some(query) = query {
        result.push('?');
        result.push_str(query);
    }
    result
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/connectors/provider_test.rs"]
mod tests;
