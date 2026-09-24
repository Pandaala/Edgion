/// Build a [`WebhookRetry`](edgion_resources::resources::common::external_http::WebhookRetry)
/// with the given `retry_delay_ms` base and otherwise-sane test defaults
/// (exponential backoff, 10s cap, no jitter).
#[cfg(test)]
pub(crate) fn tests_retry(base: u64) -> edgion_resources::resources::common::external_http::WebhookRetry {
    use edgion_resources::resources::common::external_http::{BackoffPolicy, WebhookRetry};
    WebhookRetry {
        max_retries: 3,
        retry_delay_ms: base,
        retry_on_timeout: None,
        retry_on_connect_error: true,
        retry_on_status: vec![],
        backoff_policy: BackoffPolicy::Exponential,
        max_delay_ms: 10_000,
        jitter: false,
        honor_retry_after: false,
        retry_on_body_failure: false,
    }
}
