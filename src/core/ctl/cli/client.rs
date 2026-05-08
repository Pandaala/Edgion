use anyhow::{bail, Context, Result};
use reqwest::{Client, Response};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

use super::TargetType;

/// Default server ports for different targets
const DEFAULT_PORT_CENTER: u16 = 5800;
const DEFAULT_PORT_SERVER: u16 = 5800;
const DEFAULT_PORT_CLIENT: u16 = 5900;

/// EdgionClient for interacting with Controller/Gateway APIs
pub struct EdgionClient {
    client: Client,
    base_url: String,
    target: TargetType,
    #[allow(dead_code)]
    socket_path: Option<PathBuf>,
    token: Option<Zeroizing<String>>,
    controller_config: Option<String>, // for auto-login on 401
}

impl EdgionClient {
    /// Create a new client with target type
    pub fn new(
        target: TargetType,
        server: Option<String>,
        socket: Option<PathBuf>,
        controller_config: Option<String>,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0) // Disable connection pooling
            .build()
            .context("Failed to create HTTP client")?;

        // Determine base URL based on target and server option
        let base_url = server.unwrap_or_else(|| {
            let port = match target {
                TargetType::Center => DEFAULT_PORT_CENTER,
                TargetType::Server => DEFAULT_PORT_SERVER,
                TargetType::Client => DEFAULT_PORT_CLIENT,
            };
            format!("http://localhost:{}", port)
        });

        // Load saved token (if any) from ~/.edgion/token
        let token = super::token::read_token();

        Ok(Self {
            client,
            base_url,
            target,
            socket_path: socket,
            token,
            controller_config,
        })
    }

    /// Ensure we have a valid token. Called before first API request.
    /// Flow: (1) query auth/status, (2) dispatch on AuthStatus, (3) auto-login if needed.
    pub async fn ensure_auth(&mut self) -> Result<()> {
        use crate::core::common::local_auth::{AuthStatus, AuthStatusResponse};

        let status_url = format!("{}/api/v1/auth/status", self.base_url);
        let resp = self
            .client
            .get(&status_url)
            .send()
            .await
            .with_context(|| format!("cannot reach controller at {} (network/TLS error)", self.base_url))?;

        let status_code = resp.status();
        if !status_code.is_success() {
            bail!(
                "/auth/status returned HTTP {} -- controller is too old or misconfigured",
                status_code
            );
        }

        let wrapper: serde_json::Value = resp
            .json()
            .await
            .context("/auth/status response is not valid JSON (schema unrecognized)")?;
        let data = wrapper
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("/auth/status missing `data` field"))?;
        let status: AuthStatusResponse =
            serde_json::from_value(data).context("/auth/status schema unrecognized -- CLI/controller version mismatch")?;

        match status.status {
            AuthStatus::Disabled => return Ok(()),
            AuthStatus::Initializing => {
                bail!("controller authentication subsystem initializing, please retry later");
            }
            AuthStatus::Ready => {}
        }

        if !status.auth_required {
            return Ok(());
        }

        let has_local = status.active_providers.iter().any(|p| p == "local");
        let has_oidc = status.active_providers.iter().any(|p| p == "oidc");
        if !has_local && !has_oidc {
            bail!(
                "/auth/status reported Ready but no known provider in active_providers: {:?}",
                status.active_providers
            );
        }

        // If we have a cached token, validate it — it may be stale if the controller restarted.
        if self.token.is_some() {
            let me_url = format!("{}/api/v1/auth/me", self.base_url);
            let still_valid = self
                .send_request(self.client.get(&me_url), &me_url)
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if still_valid {
                return Ok(());
            }
            // Token is stale (e.g., controller restarted with new jwt_secret) — clear and re-auth
            eprintln!("Cached token is no longer valid, re-authenticating...");
            self.token = None;
            super::token::delete_token();
        }

        if has_local {
            self.auto_login_local().await
        } else {
            self.auto_login_oidc().await
        }
    }

    /// Attempt auto-login using local credentials detected from the controller config file.
    async fn auto_login_local(&mut self) -> Result<()> {
        if let Some((username, password)) = super::auto_auth::detect_credentials(self.controller_config.as_deref()) {
            match self.do_login(&username, password.as_str()).await {
                Ok(token) => {
                    super::token::save_token(token.as_str())?;
                    self.token = Some(token);
                    eprintln!("Auto-login successful (credentials from controller config).");
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!("Auto-login failed: {}", e);
                }
            }
        }

        anyhow::bail!("Authentication required. Please run: edgion-cli login -u <username> -p <password>");
    }

    /// Attempt auto-login via OIDC (not yet supported — inform the user).
    async fn auto_login_oidc(&mut self) -> Result<()> {
        anyhow::bail!(
            "Authentication required. OIDC login is not supported in the CLI. \
             Please configure local auth or use a valid bearer token."
        );
    }

    /// Perform login and return the token wrapped in `Zeroizing` so its buffer is
    /// wiped on drop.
    async fn do_login(&self, username: &str, password: &str) -> Result<Zeroizing<String>> {
        let url = format!("{}/api/v1/auth/login", self.base_url);
        let body = serde_json::json!({
            "username": username,
            "password": password,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Login request failed: {}", e))?;

        if !resp.status().is_success() {
            anyhow::bail!("Login failed: HTTP {}", resp.status());
        }

        let data: serde_json::Value = resp.json().await?;
        let token = data["data"]["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid login response: missing token"))?
            .to_string();

        Ok(Zeroizing::new(token))
    }

    /// Get the target type
    pub fn target(&self) -> TargetType {
        self.target
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
    async fn send_request(&self, mut request: reqwest::RequestBuilder, url: &str) -> Result<Response> {
        if let Some(ref token) = self.token {
            // Borrow through Zeroizing<String> → &str; the formatted header String lives
            // only for this call, so the credential is not pinned in a long-lived field.
            request = request.header("Authorization", format!("Bearer {}", token.as_str()));
        }
        request.send().await.map_err(|e| {
            let error_msg = format_request_error(url, &e, self.target);
            anyhow::anyhow!("{}", error_msg)
        })
    }

    /// GET request - list all resources of a kind (cross-namespace)
    pub async fn list_all(&self, kind: &str) -> Result<Response> {
        let url = match self.target {
            TargetType::Center => self.build_url(&format!("/api/v1/namespaced/{}", kind)),
            TargetType::Server => self.build_url(&format!("/configserver/{}/list", kind)),
            TargetType::Client => self.build_url(&format!("/configclient/{}/list", kind)),
        };
        let request = self.client.get(&url);
        self.send_request(request, &url).await
    }

    /// GET request - list resources in a namespace
    /// For center target: uses server-side filtering
    /// For server/client targets: this method should not be called directly,
    /// use list_all() and filter on client side instead
    pub async fn list_namespaced(&self, kind: &str, namespace: &str) -> Result<Response> {
        match self.target {
            TargetType::Center => {
                let url = self.build_url(&format!("/api/v1/namespaced/{}/{}", kind, namespace));
                let request = self.client.get(&url);
                self.send_request(request, &url).await
            }
            // For server/client, we fetch all and filter on client side
            // This is handled in get.rs, but we provide a fallback here
            TargetType::Server | TargetType::Client => self.list_all(kind).await,
        }
    }

    /// GET request - get a specific resource
    pub async fn get(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = match self.target {
            TargetType::Center => {
                if let Some(ns) = namespace {
                    self.build_url(&format!("/api/v1/namespaced/{}/{}/{}", kind, ns, name))
                } else {
                    self.build_url(&format!("/api/v1/cluster/{}/{}", kind, name))
                }
            }
            TargetType::Server => {
                let mut path = format!("/configserver/{}?name={}", kind, name);
                if let Some(ns) = namespace {
                    path.push_str(&format!("&namespace={}", ns));
                }
                self.build_url(&path)
            }
            TargetType::Client => {
                let mut path = format!("/configclient/{}?name={}", kind, name);
                if let Some(ns) = namespace {
                    path.push_str(&format!("&namespace={}", ns));
                }
                self.build_url(&path)
            }
        };

        let request = self.client.get(&url);
        self.send_request(request, &url).await
    }

    /// POST request - create a resource (center target only)
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

    /// PUT request - update a resource (center target only)
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

    /// DELETE request - delete a resource (center target only)
    pub async fn delete(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<Response> {
        let url = if let Some(ns) = namespace {
            self.build_url(&format!("/api/v1/namespaced/{}/{}/{}", kind, ns, name))
        } else {
            self.build_url(&format!("/api/v1/cluster/{}/{}", kind, name))
        };

        let request = self.client.delete(&url);
        self.send_request(request, &url).await
    }

    /// POST request - reload all resources (center target only)
    pub async fn reload(&self) -> Result<Response> {
        let url = self.build_url("/api/v1/reload");
        let request = self.client.post(&url);
        self.send_request(request, &url).await
    }
}

/// Format network error with detailed diagnostics
fn format_request_error(url: &str, error: &reqwest::Error, target: TargetType) -> String {
    let mut msg = format!("Request to {} failed\n", url);

    // Determine error type and provide specific hints
    let error_string = error.to_string().to_lowercase();
    let hint = if error_string.contains("connection refused") {
        let component = match target {
            TargetType::Center | TargetType::Server => "controller",
            TargetType::Client => "gateway",
        };
        Some(format!(
            "Connection refused - {} is likely not running on this address",
            component
        ))
    } else if error.is_timeout() {
        Some("Request timed out - server may be overloaded or unresponsive".to_string())
    } else if error.is_connect() {
        if error_string.contains("dns") || error_string.contains("resolve") {
            Some("DNS resolution failed - check if the hostname is correct".to_string())
        } else if error_string.contains("no route") || error_string.contains("network is unreachable") {
            Some("No route to host - check network configuration".to_string())
        } else {
            Some("Connection failed - check if the server address is correct".to_string())
        }
    } else if error.is_request() {
        Some("Request error - check the request parameters".to_string())
    } else {
        None
    };

    // Add connection failure section with target-specific hint
    let component = match target {
        TargetType::Center | TargetType::Server => "controller",
        TargetType::Client => "gateway",
    };
    msg.push_str("\nConnection failed:\n");
    msg.push_str(&format!("  - Is the {} running?\n", component));
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

#[cfg(test)]
mod ensure_auth_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build an `EdgionClient` pointed at `base_url` with no cached token.
    fn make_client(base_url: String) -> EdgionClient {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .connect_timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("reqwest client");
        EdgionClient {
            client,
            base_url,
            target: crate::core::ctl::cli::TargetType::Center,
            socket_path: None,
            token: None,
            controller_config: None,
        }
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        let r = client.ensure_auth().await;
        assert!(r.is_err(), "404 must bail, got {:?}", r);
        let msg = format!("{:?}", r.unwrap_err());
        assert!(
            msg.contains("404") || msg.contains("too old"),
            "err message should mention 404 / too old: {msg}"
        );
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_ready_with_empty_providers() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": true,
                "active_providers": [],
                "status": "ready",
            }
        });
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        let r = client.ensure_auth().await;
        assert!(r.is_err());
        let msg = format!("{:?}", r.unwrap_err());
        assert!(
            msg.contains("no known provider"),
            "err message should mention unknown provider: {msg}"
        );
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        assert!(client.ensure_auth().await.is_err());
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_oidc_only_ready() {
        // CLI can't do OIDC login interactively — must bail with an actionable message.
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": true,
                "active_providers": ["oidc"],
                "status": "ready"
            }
        });
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        let r = client.ensure_auth().await;
        assert!(r.is_err(), "OIDC-only Ready should bail in CLI");
        let msg = format!("{:?}", r.unwrap_err()).to_lowercase();
        assert!(
            msg.contains("oidc") || msg.contains("bearer token"),
            "err should mention OIDC limitation: {msg}"
        );
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_initializing() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": true,
                "active_providers": [],
                "status": "initializing"
            }
        });
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        let r = client.ensure_auth().await;
        assert!(r.is_err());
        let msg = format!("{:?}", r.unwrap_err());
        assert!(
            msg.to_lowercase().contains("initializ"),
            "expected 'initializing' in err: {}",
            msg
        );
    }

    #[tokio::test]
    async fn ensure_auth_continues_on_disabled() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": false,
                "active_providers": [],
                "status": "disabled"
            }
        });
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        let r = client.ensure_auth().await;
        assert!(r.is_ok(), "disabled should continue, got {:?}", r);
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;
        let mut client = make_client(server.uri());
        assert!(client.ensure_auth().await.is_err());
    }

    #[tokio::test]
    async fn ensure_auth_bails_on_unreachable() {
        let mut client = make_client("http://127.0.0.1:1".to_string());
        assert!(client.ensure_auth().await.is_err());
    }
}
