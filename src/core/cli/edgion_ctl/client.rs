use anyhow::{Context, Result};
use reqwest::{Client, Response};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

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
            .timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(0) // Disable connection pooling
            .build()
            .context("Failed to create HTTP client")?;

        // Determine base URL
        let base_url = if let Some(url) = server {
            url
        } else {
            "http://localhost:8080".to_string()
        };

        Ok(Self {
            client,
            base_url,
            socket_path: socket,
        })
    }

    /// Build full URL for API endpoint
    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// GET request - list all resources of a kind (cross-namespace)
    pub async fn list_all(&self, kind: &str) -> Result<Response> {
        let url = self.build_url(&format!("/api/{}", kind));
        self.client.get(&url).send().await.context("Failed to send GET request")
    }

    /// GET request - list resources in a namespace
    pub async fn list_namespaced(&self, kind: &str, namespace: &str) -> Result<Response> {
        let url = self.build_url(&format!("/api/namespaces/{}/{}", namespace, kind));
        self.client.get(&url).send().await.context("Failed to send GET request")
    }

    /// GET request - get a specific resource
    pub async fn get(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/namespaces/{}/{}/{}", ns, kind, name))
        } else {
            self.build_url(&format!("/api/cluster/{}/{}", kind, name))
        };

        self.client.get(&url).send().await.context("Failed to send GET request")
    }

    /// POST request - create a resource
    pub async fn create(&self, kind: &str, namespace: Option<&str>, body: String) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/namespaces/{}/{}", ns, kind))
        } else {
            self.build_url(&format!("/api/cluster/{}", kind))
        };

        self.client
            .post(&url)
            .header("Content-Type", "application/yaml")
            .body(body)
            .send()
            .await
            .context("Failed to send POST request")
    }

    /// PUT request - update a resource
    pub async fn update(&self, kind: &str, namespace: Option<&str>, name: &str, body: String) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/namespaces/{}/{}/{}", ns, kind, name))
        } else {
            self.build_url(&format!("/api/cluster/{}/{}", kind, name))
        };

        self.client
            .put(&url)
            .header("Content-Type", "application/yaml")
            .body(body)
            .send()
            .await
            .context("Failed to send PUT request")
    }

    /// DELETE request - delete a resource
    pub async fn delete(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/namespaces/{}/{}/{}", ns, kind, name))
        } else {
            self.build_url(&format!("/api/cluster/{}/{}", kind, name))
        };

        self.client
            .delete(&url)
            .send()
            .await
            .context("Failed to send DELETE request")
    }

    /// POST request - reload all resources
    pub async fn reload(&self) -> Result<Response> {
        let url = self.build_url("/api/reload");
        self.client
            .post(&url)
            .send()
            .await
            .context("Failed to send reload request")
    }
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
