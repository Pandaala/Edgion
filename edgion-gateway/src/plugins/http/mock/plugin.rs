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
use bytes::Bytes;
use std::{collections::HashMap, time::Duration};
use tokio::time::sleep;

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::MockConfig;

const PLUGIN_NAME: &str = "Mock";

pub struct Mock {
    status_code: u16,
    /// Prepared once per generation so the per-request clone is a refcount
    /// bump instead of a full copy of the configured response body.
    body: Option<Bytes>,
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
            body: config.body.clone().map(Bytes::from),
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
#[path = "../../../../tests/unit/plugins/http/mock/plugin_test.rs"]
mod tests;
