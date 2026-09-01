//! Mock plugin implementation
//!
//! This plugin returns predefined mock responses without forwarding to upstream services.
//! Ideal for API prototyping, testing, and simulating various response scenarios.
//!
//! ## Use Cases:
//! - API development and prototyping before backend is ready
//! - Testing error handling (4xx, 5xx responses)
//! - Simulating slow network with delays
//! - Creating test environments without real backends

use async_trait::async_trait;
use std::{collections::HashMap, time::Duration};
use tokio::time::sleep;

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::MockConfig;

const PLUGIN_NAME: &str = "Mock";

pub struct Mock {
    status_code: u16,
    body: Option<String>,
    headers: Option<HashMap<String, String>>,
    content_type: String,
    delay: Option<Duration>,
    terminate: bool,
    status_log: String,
}

impl Mock {
    /// Create a new Mock plugin from configuration
    pub fn new(config: &MockConfig) -> Self {
        Mock {
            status_code: config.status_code,
            body: config.body.clone(),
            headers: config.headers.clone(),
            content_type: config.content_type.clone(),
            delay: config.delay.map(Duration::from_millis),
            terminate: config.terminate,
            status_log: format!("mock served {}", config.status_code),
        }
    }
}

#[async_trait]
impl RequestFilter for Mock {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if let Some(delay) = self.delay {
            sleep(delay).await;
        }

        if let Some(ref headers) = self.headers {
            for (name, value) in headers {
                let _ = session.set_response_header(name, value);
            }
        }

        let _ = session.set_response_header("Content-Type", &self.content_type);

        if self.terminate {
            // Return mock response (terminates request, no upstream call)
            plugin_log.push(&self.status_log);
            PluginRunningResult::ErrResponse {
                status: self.status_code,
                body: self.body.clone(),
            }
        } else {
            // Non-terminating (e.g., for logging/headers only)
            plugin_log.push("mock headers set");
            PluginRunningResult::GoodNext
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;

    fn config(status_code: u16, body: &str, terminate: bool) -> MockConfig {
        MockConfig {
            status_code,
            body: Some(body.to_string()),
            headers: Some(HashMap::from([("X-Mock".to_string(), "yes".to_string())])),
            content_type: "text/plain".to_string(),
            delay: None,
            terminate,
        }
    }

    #[test]
    fn name_is_static_across_instances() {
        let first = Mock::new(&MockConfig::default());
        let second = Mock::new(&MockConfig::default());

        assert_eq!(first.name(), PLUGIN_NAME);
        assert_eq!(second.name(), PLUGIN_NAME);
        assert!(std::ptr::eq(first.name(), second.name()));
    }

    #[test]
    fn prepared_generations_do_not_retain_or_share_caller_config() {
        let mut first_config = config(201, "first", true);
        first_config.delay = Some(25);
        let first = Mock::new(&first_config);

        first_config.status_code = 503;
        first_config.body = Some("mutated".to_string());
        first_config
            .headers
            .as_mut()
            .unwrap()
            .insert("X-Later".to_string(), "changed".to_string());
        first_config.content_type = "application/json".to_string();
        first_config.delay = Some(99);
        first_config.terminate = false;

        let second = Mock::new(&config(202, "second", false));

        assert_eq!(first.status_code, 201);
        assert_eq!(first.body.as_deref(), Some("first"));
        assert_eq!(first.headers.as_ref().unwrap().len(), 1);
        assert_eq!(first.content_type, "text/plain");
        assert_eq!(first.delay, Some(Duration::from_millis(25)));
        assert!(first.terminate);
        assert_eq!(first.status_log, "mock served 201");

        assert_eq!(second.status_code, 202);
        assert_eq!(second.body.as_deref(), Some("second"));
        assert!(!second.terminate);
        assert_eq!(second.status_log, "mock served 202");
    }

    #[tokio::test]
    async fn terminating_response_preserves_header_order_result_and_log() {
        let mock = Mock::new(&config(201, "created", true));
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-Mock" && value == "yes")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == "Content-Type" && value == "text/plain")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new(PLUGIN_NAME);
        let result = mock.run_request(&mut session, &mut log).await;

        match result {
            PluginRunningResult::ErrResponse { status, body } => {
                assert_eq!(status, 201);
                assert_eq!(body.as_deref(), Some("created"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
        assert_eq!(log.render_for_test(), "mock served 201");
    }

    #[tokio::test]
    async fn non_terminating_response_sets_headers_then_continues() {
        let mock = Mock::new(&config(503, "ignored", false));
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-Mock" && value == "yes")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == "Content-Type" && value == "text/plain")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new(PLUGIN_NAME);
        let result = mock.run_request(&mut session, &mut log).await;

        assert!(matches!(result, PluginRunningResult::GoodNext));
        assert_eq!(log.render_for_test(), "mock headers set");
    }

    #[tokio::test(start_paused = true)]
    async fn delay_completes_before_response_headers_are_set() {
        let mut mock_config = config(200, "delayed", true);
        mock_config.delay = Some(100);
        let mock = Mock::new(&mock_config);
        let mut session = MockPluginSession::new();
        let start = tokio::time::Instant::now();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-Mock" && value == "yes")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_, _| {
                assert_eq!(
                    tokio::time::Instant::now().duration_since(start),
                    Duration::from_millis(100)
                );
                Ok(())
            });
        session
            .expect_set_response_header()
            .withf(|name, value| name == "Content-Type" && value == "text/plain")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_, _| {
                assert_eq!(
                    tokio::time::Instant::now().duration_since(start),
                    Duration::from_millis(100)
                );
                Ok(())
            });

        let mut log = PluginLog::new(PLUGIN_NAME);
        let result = {
            let run = mock.run_request(&mut session, &mut log);
            tokio::pin!(run);

            tokio::select! {
                result = &mut run => panic!("mock completed before its delay: {result:?}"),
                () = sleep(Duration::from_millis(99)) => {}
            }
            assert_eq!(
                tokio::time::Instant::now().duration_since(start),
                Duration::from_millis(99)
            );

            run.await
        };
        assert_eq!(
            tokio::time::Instant::now().duration_since(start),
            Duration::from_millis(100)
        );
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 200, .. }));
        assert_eq!(log.render_for_test(), "mock served 200");
    }
}
