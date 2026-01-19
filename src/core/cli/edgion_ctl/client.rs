use anyhow::{Context, Result};
use reqwest::{Client, Response};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Default server address for edgion-ctl
const DEFAULT_SERVER: &str = "http://localhost:5800";

/// EdgionClient for interacting with the Controller API
pub struct EdgionClient {
    client: Client,
    base_url: String,
    #[allow(dead_code)]
    socket_path: Option<PathBuf>,
}

impl EdgionClient {
    /// Create a new client with default settings
    /// Tries Unix Socket first, falls back to HTTP
    pub fn new(server: Option<String>, socket: Option<PathBuf>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0) // Disable connection pooling
            .build()
            .context("Failed to create HTTP client")?;

        // Determine base URL
        let base_url = server.unwrap_or_else(|| DEFAULT_SERVER.to_string());

        Ok(Self {
            client,
            base_url,
            socket_path: socket,
        })
    }

    /// Get the base URL for connection hints
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Build full URL for API endpoint
    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Send HTTP request with detailed error handling
    async fn send_request(&self, request: reqwest::RequestBuilder, url: &str) -> Result<Response> {
        request.send().await.map_err(|e| {
            let error_msg = format_request_error(url, &e);
            anyhow::anyhow!("{}", error_msg)
        })
    }

    /// GET request - list all resources of a kind (cross-namespace)
    /// Uses K8s-style API: /api/v1/namespaced/{kind}
    pub async fn list_all(&self, kind: &str) -> Result<Response> {
        let url = self.build_url(&format!("/api/v1/namespaced/{}", kind));
        let request = self.client.get(&url);
        self.send_request(request, &url).await
    }

    /// GET request - list resources in a namespace
    /// Uses K8s-style API: /api/v1/namespaced/{kind}/{namespace}
    pub async fn list_namespaced(&self, kind: &str, namespace: &str) -> Result<Response> {
        let url = self.build_url(&format!("/api/v1/namespaced/{}/{}", kind, namespace));
        let request = self.client.get(&url);
        self.send_request(request, &url).await
    }

    /// GET request - get a specific resource
    /// Namespaced: /api/v1/namespaced/{kind}/{namespace}/{name}
    /// Cluster: /api/v1/cluster/{kind}/{name}
    pub async fn get(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/v1/namespaced/{}/{}/{}", kind, ns, name))
        } else {
            self.build_url(&format!("/api/v1/cluster/{}/{}", kind, name))
        };

        let request = self.client.get(&url);
        self.send_request(request, &url).await
    }

    /// POST request - create a resource
    /// Namespaced: /api/v1/namespaced/{kind}/{namespace}
    /// Cluster: /api/v1/cluster/{kind}
    pub async fn create(&self, kind: &str, namespace: Option<&str>, body: String) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/v1/namespaced/{}/{}", kind, ns))
        } else {
            self.build_url(&format!("/api/v1/cluster/{}", kind))
        };

        let request = self
            .client
            .post(&url)
            .header("Content-Type", "application/yaml")
            .body(body);
        self.send_request(request, &url).await
    }

    /// PUT request - update a resource
    /// Namespaced: /api/v1/namespaced/{kind}/{namespace}/{name}
    /// Cluster: /api/v1/cluster/{kind}/{name}
    pub async fn update(&self, kind: &str, namespace: Option<&str>, name: &str, body: String) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/v1/namespaced/{}/{}/{}", kind, ns, name))
        } else {
            self.build_url(&format!("/api/v1/cluster/{}/{}", kind, name))
        };

        let request = self
            .client
            .put(&url)
            .header("Content-Type", "application/yaml")
            .body(body);
        self.send_request(request, &url).await
    }

    /// DELETE request - delete a resource
    /// Namespaced: /api/v1/namespaced/{kind}/{namespace}/{name}
    /// Cluster: /api/v1/cluster/{kind}/{name}
    pub async fn delete(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/v1/namespaced/{}/{}/{}", kind, ns, name))
        } else {
            self.build_url(&format!("/api/v1/cluster/{}/{}", kind, name))
        };

        let request = self.client.delete(&url);
        self.send_request(request, &url).await
    }

    /// POST request - reload all resources
    pub async fn reload(&self) -> Result<Response> {
        let url = self.build_url("/api/v1/reload");
        let request = self.client.post(&url);
        self.send_request(request, &url).await
    }
}

/// Format network error with detailed diagnostics
fn format_request_error(url: &str, error: &reqwest::Error) -> String {
    let mut msg = format!("Request to {} failed\n", url);

    // Determine error type and provide specific hints
    let error_string = error.to_string().to_lowercase();
    let hint = if error_string.contains("connection refused") {
        Some("Connection refused - controller is likely not running on this address")
    } else if error.is_timeout() {
        Some("Request timed out - server may be overloaded or unresponsive")
    } else if error.is_connect() {
        if error_string.contains("dns") || error_string.contains("resolve") {
            Some("DNS resolution failed - check if the hostname is correct")
        } else if error_string.contains("no route") || error_string.contains("network is unreachable") {
            Some("No route to host - check network configuration")
        } else {
            Some("Connection failed - check if the server address is correct")
        }
    } else if error.is_request() {
        Some("Request error - check the request parameters")
    } else {
        None
    };

    // Add connection failure section
    msg.push_str("\nConnection failed:\n");
    msg.push_str("  - Is the controller running?\n");
    msg.push_str("  - Check if the server address is correct\n");
    msg.push_str(&format!("  - Try: curl -v {}\n", url));

    // Add error details
    msg.push_str(&format!("\nDetails: {}\n", error));

    // Add hint if available
    if let Some(h) = hint {
        msg.push_str(&format!("\nHint: {}", h));
    }

    msg
}

/// Handle API response and extract success/error message
pub async fn handle_response(resp: Response) -> Result<String> {
    let status = resp.status();
    let body = resp.text().await.context("Failed to read response body")?;

    if status.is_success() {
        // Try to parse as JSON to extract message
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            if let Some(data) = json.get("data") {
                return Ok(data.to_string().trim_matches('"').to_string());
            }
        }
        Ok(body)
    } else {
        // Try to extract error message
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            if let Some(error) = json.get("error") {
                anyhow::bail!("API error: {}", error.as_str().unwrap_or("Unknown error"));
            }
        }
        anyhow::bail!("Request failed with status {}: {}", status, body);
    }
}

/// Parse response as JSON Value
pub async fn parse_json_response(resp: Response) -> Result<Value> {
    let status = resp.status();
    let body = resp.text().await.context("Failed to read response body")?;

    if !status.is_success() {
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            if let Some(error) = json.get("error") {
                anyhow::bail!("API error: {}", error.as_str().unwrap_or("Unknown error"));
            }
        }
        anyhow::bail!("Request failed with status {}: {}", status, body);
    }

    serde_json::from_str(&body).context("Failed to parse JSON response")
}
