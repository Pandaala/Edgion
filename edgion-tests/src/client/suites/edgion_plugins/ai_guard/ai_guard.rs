//! Focused end-to-end coverage for request-side AI prompt guarding.
//!
//! These cases assert the client-visible contract: a clean prompt reaches the
//! provider unchanged, a policy hit is refused in the configured shape, and a
//! masked prompt is still a valid request the provider answers normally.
//!
//! Whether the masked bytes are exactly what the provider received is asserted
//! at the unit level instead (the guard's edits are checked against the body
//! seam directly). Proving it here would mean adding a field to the shared AI
//! provider observation, which would break the Guardrail suite's byte goldens
//! for no additional safety.

use std::time::Instant;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::json;

pub struct AiGuardTestSuite;

const PASS_HOST: &str = "ai-guard-pass.example.com";
const REJECT_ERROR_HOST: &str = "ai-guard-reject-error.example.com";
const CONTENT_FILTER_HOST: &str = "ai-guard-content-filter.example.com";
const MASK_HOST: &str = "ai-guard-mask.example.com";
const PATH: &str = "/v1/chat/completions";

/// A prompt with nothing any configured rule matches.
const CLEAN_PROMPT: &str = "summarize this paragraph";
/// Matches the `forbidden-token` regex rule.
const FORBIDDEN_PROMPT: &str = "please echo the forbidden-token now";
/// Matches the builtin AWS access-key detector.
const CREDENTIAL_PROMPT: &str = "my key is AKIAIOSFODNN7EXAMPLE keep it safe";
/// Matches the builtin email detector.
const EMAIL_PROMPT: &str = "contact me at alice@example.com tomorrow";

async fn chat(ctx: &TestContext, host: &str, prompt: &str, stream: bool) -> Result<reqwest::Response, String> {
    ctx.http_client
        .post(format!("{}{PATH}", ctx.http_url()))
        .header("host", host)
        .header("connection", "close")
        .json(&json!({
            "model": "fixture-model",
            "stream": stream,
            "messages": [{"role": "user", "content": prompt}],
        }))
        .send()
        .await
        .map_err(|error| format!("request to {host} failed: {error}"))
}

impl AiGuardTestSuite {
    /// A prompt no rule matches must reach the provider and be answered.
    fn clean_prompt_reaches_the_provider() -> TestCase {
        TestCase::new(
            "clean_prompt_reaches_the_provider",
            "A prompt matching no rule is forwarded and answered normally",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match chat(&ctx, PASS_HOST, CLEAN_PROMPT, true).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    let body = match response.text().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    if !body.contains("stream-fixture") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected the provider fixture stream, got: {}",
                                &body[..body.len().min(300)]
                            ),
                        );
                    }
                    TestResult::passed_with_message(start.elapsed(), "provider answered the clean prompt".to_string())
                })
            },
        )
    }

    /// `rejectionMode: error` renders the fixed OpenAI error envelope and
    /// never discloses the rule or the matched text.
    fn policy_reject_renders_the_fixed_error_envelope() -> TestCase {
        TestCase::new(
            "policy_reject_renders_the_fixed_error_envelope",
            "A matched reject rule returns 403 with a sanitized content-filter error",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match chat(&ctx, REJECT_ERROR_HOST, FORBIDDEN_PROMPT, false).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::FORBIDDEN {
                        return TestResult::failed(start.elapsed(), format!("expected 403, got {}", response.status()));
                    }
                    let body = match response.text().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    for expected in ["content_filter_error", "content_policy_violation"] {
                        if !body.contains(expected) {
                            return TestResult::failed(start.elapsed(), format!("missing {expected} in {body}"));
                        }
                    }
                    // The rule name and the offending prompt must never leak.
                    for leaked in ["tripwire", "forbidden-token"] {
                        if body.contains(leaked) {
                            return TestResult::failed(start.elapsed(), format!("refusal leaked {leaked}"));
                        }
                    }
                    TestResult::passed_with_message(start.elapsed(), "sanitized 403 refusal".to_string())
                })
            },
        )
    }

    /// `rejectionMode: contentFilter` degrades instead of erroring.
    fn content_filter_mode_returns_a_degraded_completion() -> TestCase {
        TestCase::new(
            "content_filter_mode_returns_a_degraded_completion",
            "contentFilter mode returns 200 with finish_reason content_filter",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match chat(&ctx, CONTENT_FILTER_HOST, CREDENTIAL_PROMPT, false).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    let body = match response.text().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    if !body.contains(r#""finish_reason":"content_filter""#) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected a content_filter completion, got {body}"),
                        );
                    }
                    if body.contains("AKIAIOSFODNN7EXAMPLE") {
                        return TestResult::failed(start.elapsed(), "refusal echoed the credential".to_string());
                    }
                    TestResult::passed_with_message(start.elapsed(), "degraded completion returned".to_string())
                })
            },
        )
    }

    /// A masked prompt must remain a valid request the provider answers.
    fn a_masked_prompt_is_still_served() -> TestCase {
        TestCase::new(
            "a_masked_prompt_is_still_served",
            "A masked prompt stays a valid canonical request and is answered",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match chat(&ctx, MASK_HOST, EMAIL_PROMPT, true).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected 200 after masking, got {}", response.status()),
                        );
                    }
                    let body = match response.text().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    if !body.contains("stream-fixture") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("masked request was not served: {}", &body[..body.len().min(300)]),
                        );
                    }
                    TestResult::passed_with_message(start.elapsed(), "masked request served".to_string())
                })
            },
        )
    }

    /// A request that is not a chat completion must pass through untouched,
    /// so a guard cannot fail-close ordinary traffic.
    fn a_non_chat_completion_request_is_untouched() -> TestCase {
        TestCase::new(
            "a_non_chat_completion_request_is_untouched",
            "A GET on a guarded route is not guarded and not refused",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}{PATH}", ctx.http_url()))
                        .header("host", REJECT_ERROR_HOST)
                        .header("connection", "close")
                        .send()
                        .await;
                    let response = match response {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("request failed: {error}")),
                    };
                    // The guard must not be the thing that refuses it: a 403
                    // content-filter refusal here would mean ordinary traffic
                    // is being fail-closed by a prompt guard.
                    if response.status() == reqwest::StatusCode::FORBIDDEN {
                        let body = response.text().await.unwrap_or_default();
                        if body.contains("content_policy_violation") {
                            return TestResult::failed(
                                start.elapsed(),
                                "a non-chat-completion request was refused by the guard".to_string(),
                            );
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "non-chat-completion request was not guarded".to_string(),
                    )
                })
            },
        )
    }
}

impl TestSuite for AiGuardTestSuite {
    fn name(&self) -> &str {
        "AiGuard"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::clean_prompt_reaches_the_provider(),
            Self::policy_reject_renders_the_fixed_error_envelope(),
            Self::content_filter_mode_returns_a_degraded_completion(),
            Self::a_masked_prompt_is_still_served(),
            Self::a_non_chat_completion_request_is_untouched(),
        ]
    }
}
