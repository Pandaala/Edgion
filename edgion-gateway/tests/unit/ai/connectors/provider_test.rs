use super::*;

#[test]
fn provider_uri_uses_canonical_path_and_query() {
    assert_eq!(
        provider_uri(
            AiProvider::OpenAICompatible,
            "/openai/v1",
            "/chat/completions",
            Some("trace=1")
        ),
        "/openai/v1/chat/completions?trace=1"
    );
    assert_eq!(
        provider_uri(AiProvider::Anthropic, "/native", "/chat/completions", Some("ignored=1")),
        "/native/v1/messages"
    );
}
