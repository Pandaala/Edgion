//! Access Log Client for Integration Testing
//!
//! Provides utilities to query the gateway's Access Log Store via Admin API.
//! Used in integration tests to verify request processing behavior.
//!
//! The Access Log Store is only available when the gateway is started with
//! `--integration-testing-mode`.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Convert a readable test marker into a stable, nonzero W3C trace ID.
/// Existing valid IDs are unchanged, so callers may normalize more than once.
pub fn trace_id_for_marker(marker: impl AsRef<str>) -> String {
    use sha2::{Digest, Sha256};
    let marker = marker.as_ref();
    if marker.len() == 32
        && marker
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && marker.bytes().any(|byte| byte != b'0')
    {
        return marker.to_owned();
    }
    let digest = Sha256::digest(marker.as_bytes());
    let mut id = format!("{:032x}", u128::from_be_bytes(digest[..16].try_into().unwrap()));
    if id.bytes().all(|byte| byte == b'0') {
        id.replace_range(31..32, "1");
    }
    id
}

/// Carry a test marker through the gateway's canonical W3C identity path.
pub fn traceparent_for_marker(marker: impl AsRef<str>) -> String {
    format!("00-{}-0123456789abcdef-00", trace_id_for_marker(marker))
}

#[cfg(test)]
mod marker_tests {
    use super::{trace_id_for_marker, traceparent_for_marker};

    #[test]
    fn markers_preserve_repeated_lookup_and_valid_w3c_identity() {
        let id = trace_id_for_marker("repeat-request-marker");
        assert_eq!(id, trace_id_for_marker("repeat-request-marker"));
        assert_eq!(id, trace_id_for_marker(&id));
        assert_ne!(id, trace_id_for_marker("another-request-marker"));
        assert_eq!(id.len(), 32);
        assert!(id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_eq!(&traceparent_for_marker("repeat-request-marker")[3..35], id);
        assert_ne!(
            trace_id_for_marker("00000000000000000000000000000000"),
            "00000000000000000000000000000000"
        );
        let existing = "4bf92f3577b34da6a3ce929d0e0e4736";
        assert_eq!(trace_id_for_marker(existing), existing);
    }
}

/// Access log entry returned from the gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    /// Raw JSON value of the access log
    #[serde(flatten)]
    pub data: serde_json::Value,
}

impl AccessLogEntry {
    /// Get a field value as a string
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(|v| v.as_str())
    }

    /// Get a nested field value as a string (e.g., "request_info.host")
    pub fn get_nested_str(&self, path: &str) -> Option<&str> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = &self.data;
        for part in parts {
            current = current.get(part)?;
        }
        current.as_str()
    }

    /// Get a field value as u64
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.data.get(key).and_then(|v| v.as_u64())
    }

    /// Get the response status code
    pub fn status_code(&self) -> Option<u64> {
        self.data
            .get("request_info")
            .and_then(|ri| ri.get("status"))
            .and_then(|v| v.as_u64())
    }

    /// Get the request path
    pub fn request_path(&self) -> Option<&str> {
        self.data
            .get("request_info")
            .and_then(|ri| ri.get("path"))
            .and_then(|v| v.as_str())
    }

    /// Get the request host
    pub fn request_host(&self) -> Option<&str> {
        self.data
            .get("request_info")
            .and_then(|ri| ri.get("host"))
            .and_then(|v| v.as_str())
    }

    /// Get the matched route namespace
    pub fn route_namespace(&self) -> Option<&str> {
        self.data
            .get("match_info")
            .and_then(|mi| mi.get("rsc"))
            .and_then(|rsc| rsc.get("ns"))
            .and_then(|v| v.as_str())
    }

    /// Get the matched route name
    pub fn route_name(&self) -> Option<&str> {
        self.data
            .get("match_info")
            .and_then(|mi| mi.get("rsc"))
            .and_then(|rsc| rsc.get("name"))
            .and_then(|v| v.as_str())
    }

    /// Get the matched route kind (e.g., "HTTPRoute", "GRPCRoute")
    pub fn route_kind(&self) -> Option<&str> {
        self.data
            .get("match_info")
            .and_then(|mi| mi.get("rsc"))
            .and_then(|rsc| rsc.get("kind"))
            .and_then(|v| v.as_str())
    }

    /// Get backend name
    pub fn backend_name(&self) -> Option<&str> {
        self.data
            .get("backend_context")
            .and_then(|bc| bc.get("rsc"))
            .and_then(|rsc| rsc.get("name"))
            .and_then(|v| v.as_str())
    }

    /// Get backend namespace
    pub fn backend_namespace(&self) -> Option<&str> {
        self.data
            .get("backend_context")
            .and_then(|bc| bc.get("rsc"))
            .and_then(|rsc| rsc.get("ns"))
            .and_then(|v| v.as_str())
    }

    /// Get the number of upstream connection attempts
    pub fn upstream_count(&self) -> usize {
        self.data
            .get("backend_context")
            .and_then(|b| b.get("upstreams"))
            .and_then(|u| u.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    }

    /// Get diagnostic codes of one kind while preserving their recorded order.
    pub fn diagnostic_codes(&self, kind: &str) -> Vec<String> {
        self.data
            .get("diagnostics")
            .and_then(|diagnostics| diagnostics.as_array())
            .map(|diagnostics| {
                diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.get("kind").and_then(|value| value.as_str()) == Some(kind))
                    .filter_map(|diagnostic| {
                        diagnostic
                            .get("code")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check whether an access log contains a diagnostic with the given kind and code.
    pub fn has_diagnostic(&self, kind: &str, code: &str) -> bool {
        self.data
            .get("diagnostics")
            .and_then(|diagnostics| diagnostics.as_array())
            .is_some_and(|diagnostics| {
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.get("kind").and_then(|value| value.as_str()) == Some(kind)
                        && diagnostic.get("code").and_then(|value| value.as_str()) == Some(code)
                })
            })
    }

    /// Check if a specific plugin was executed in any stage
    pub fn has_plugin_log(&self, plugin_name: &str) -> bool {
        self.data
            .get("stage_logs")
            .and_then(|sl| sl.as_array())
            .map(|stages| {
                stages.iter().any(|stage| {
                    stage
                        .get("plugins")
                        .and_then(|p| p.as_array())
                        .map(|plugins| {
                            plugins.iter().any(|plugin| {
                                plugin
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .map(|n| n == plugin_name)
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }
}

/// API response wrapper
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

/// List response item
#[derive(Debug, Deserialize)]
pub struct AccessLogListItem {
    pub trace_id: String,
    pub stored_at_ms_ago: u64,
    pub store_count: u64,
}

/// List response
#[derive(Debug, Deserialize)]
pub struct AccessLogListResponse {
    pub success: bool,
    pub count: usize,
    pub data: Vec<AccessLogListItem>,
}

/// Testing status response
#[derive(Debug, Deserialize)]
pub struct TestingStatus {
    pub integration_testing_mode: bool,
    pub access_log_store: AccessLogStoreStatus,
}

/// Access log store status
#[derive(Debug, Deserialize)]
pub struct AccessLogStoreStatus {
    pub enabled: bool,
    pub entry_count: usize,
    pub total_stored: u64,
    pub max_capacity: usize,
    pub ttl_seconds: u64,
}

/// Client for querying the gateway's Access Log Store
pub struct AccessLogClient {
    client: Client,
    base_url: String,
    /// Bearer token for the gateway admin API. The testing endpoints are mounted
    /// on the auth-guarded admin router, so every request must carry this token.
    /// Defaults from the `EDGION_GATEWAY_ADMIN_TOKEN` env var exported by the test
    /// harness (start_all_with_conf.sh); `None` falls back to unauthenticated.
    admin_token: Option<String>,
}

impl AccessLogClient {
    /// Create a new AccessLogClient
    pub fn new(base_url: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        let admin_token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());

        Self {
            client,
            base_url,
            admin_token,
        }
    }

    /// Attach the gateway admin bearer token when one is configured.
    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.admin_token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// Create from host and port
    pub fn from_host_port(host: &str, port: u16) -> Self {
        Self::new(format!("http://{}:{}", host, port))
    }

    /// Create with default gateway admin port (12001)
    pub fn default_gateway() -> Self {
        Self::from_host_port("127.0.0.1", 12001)
    }

    /// Check if integration testing mode is enabled
    pub async fn check_status(&self) -> Result<TestingStatus> {
        let url = format!("{}/api/v1/testing/status", self.base_url);
        let response = self.authed(self.client.get(&url)).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Testing status endpoint returned HTTP {}. Is --integration-testing-mode enabled?",
                response.status()
            ));
        }

        let api_response: ApiResponse<TestingStatus> = response.json().await?;
        api_response
            .data
            .ok_or_else(|| anyhow!("No data in testing status response"))
    }

    /// Get access log by trace_id
    ///
    /// Accepts a W3C trace ID or the marker supplied to `traceparent_for_marker`.
    pub async fn get_access_log(&self, trace_id: &str) -> Result<AccessLogEntry> {
        let trace_id = trace_id_for_marker(trace_id);
        let url = format!("{}/api/v1/testing/access-log/{}", self.base_url, trace_id);
        let response = self.authed(self.client.get(&url)).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to get access log: HTTP {}", response.status()));
        }

        let api_response: ApiResponse<serde_json::Value> = response.json().await?;
        match api_response.data {
            Some(data) => Ok(AccessLogEntry { data }),
            None => Err(anyhow!(
                "Access log not found: {}",
                api_response.error.unwrap_or_default()
            )),
        }
    }

    /// Get access log with retry (waits for the log to appear)
    ///
    /// Useful when you send a request and immediately want to query the log,
    /// as there may be a slight delay before the log is stored.
    pub async fn get_access_log_with_retry(
        &self,
        trace_id: &str,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<AccessLogEntry> {
        for attempt in 0..max_retries {
            match self.get_access_log(trace_id).await {
                Ok(entry) => return Ok(entry),
                Err(_) if attempt < max_retries - 1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(anyhow!(
            "Access log not found after {} retries: {}",
            max_retries,
            trace_id
        ))
    }

    /// Delete access log by trace_id
    pub async fn delete_access_log(&self, trace_id: &str) -> Result<bool> {
        let trace_id = trace_id_for_marker(trace_id);
        let url = format!("{}/api/v1/testing/access-log/{}", self.base_url, trace_id);
        let response = self.authed(self.client.delete(&url)).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to delete access log: HTTP {}", response.status()));
        }

        let api_response: ApiResponse<String> = response.json().await?;
        Ok(api_response.success)
    }

    /// List all stored access logs
    pub async fn list_access_logs(&self) -> Result<AccessLogListResponse> {
        let url = format!("{}/api/v1/testing/access-logs", self.base_url);
        let response = self.authed(self.client.get(&url)).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to list access logs: HTTP {}", response.status()));
        }

        Ok(response.json().await?)
    }

    /// Clear all stored access logs
    pub async fn clear_access_logs(&self) -> Result<()> {
        let url = format!("{}/api/v1/testing/access-logs", self.base_url);
        let response = self.authed(self.client.delete(&url)).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to clear access logs: HTTP {}", response.status()));
        }

        Ok(())
    }
}
