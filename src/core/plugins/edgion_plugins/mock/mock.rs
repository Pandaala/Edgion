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
use tokio::time::{sleep, Duration};

use crate::core::plugins::plugin_runtime::{RequestFilter, PluginSession, PluginLog};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::MockConfig;

pub struct Mock {
    name: String,
    config: MockConfig,
}

impl Mock {
    /// Create a new Mock plugin from configuration
    pub fn new(config: &MockConfig) -> Self {
        Mock {
            name: "Mock".to_string(),
            config: config.clone(),
        }
    }
}

#[async_trait]
impl RequestFilter for Mock {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        plugin_log.add_plugin_log(&format!("Returning mock response with status {}; ", self.config.status_code));

        // Apply delay if configured
        if let Some(delay_ms) = self.config.delay {
            plugin_log.add_plugin_log(&format!("Delaying response by {}ms; ", delay_ms));
            sleep(Duration::from_millis(delay_ms)).await;
        }

        // Set custom response headers
        if let Some(ref headers) = self.config.headers {
            for (name, value) in headers {
                if let Err(e) = session.set_response_header(name, value) {
                    plugin_log.add_plugin_log(&format!("Failed to set header {}: {}; ", name, e));
                }
            }
        }

        // Set Content-Type header
        if let Err(e) = session.set_response_header("Content-Type", &self.config.content_type) {
            plugin_log.add_plugin_log(&format!("Failed to set Content-Type: {}; ", e));
        }

        // Return mock response (terminates request, no upstream call)
        PluginRunningResult::ErrResponse {
            status: self.config.status_code,
            body: self.config.body.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_creation() {
        let config = MockConfig::new(200, r#"{"message":"OK"}"#.to_string());
        let mock = Mock::new(&config);

        assert_eq!(mock.name(), "Mock");
        assert_eq!(mock.config.status_code, 200);
        assert_eq!(mock.config.body, Some(r#"{"message":"OK"}"#.to_string()));
    }
}
