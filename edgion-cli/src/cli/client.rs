use anyhow::{bail, Context, Result};
use reqwest::{Client, Response};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

use super::TargetType;

/// Append `limit` and `continue` query params to a URL.
///
/// Uses `query_pairs_mut` so values are properly URL-encoded (continue tokens are
/// base64-url-safe per K8s spec, but be defensive).
fn build_list_url_with_pagination(base: &str, limit: Option<u32>, continue_token: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(base)?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(n) = limit {
            q.append_pair("limit", &n.to_string());
        }
        if let Some(tok) = continue_token {
            q.append_pair("continue", tok);
        }
    }
    Ok(url.to_string())
}

/// Default server ports for different targets
const DEFAULT_PORT_CENTER: u16 = 12101; // Controller ConfCenter CRUD API
const DEFAULT_PORT_SERVER: u16 = 12101; // Controller ConfigServer cache
const DEFAULT_PORT_CLIENT: u16 = 12001; // Gateway admin

/// EdgionClient for interacting with Controller/Gateway APIs
pub struct EdgionClient {
    client: Client,
    base_url: String,
    target: TargetType,
    #[allow(dead_code)]
    socket_path: Option<PathBuf>,
    token: Option<Zeroizing<String>>,
}

impl EdgionClient {
    /// Create a new client with no bearer token.
    ///
    /// Authentication is token-only: when no token is resolved (`--token`,
    /// `EDGION_TOKEN`, or `~/.edgion/token`), this token-less client is created so
    /// `ensure_auth` can print a friendly "no token configured" message. With a
    /// resolved token the caller uses `with_static_bearer` instead.
    pub fn new(target: TargetType, server: Option<String>, socket: Option<PathBuf>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0) // Disable connection pooling
            .build()
            .context("Failed to create HTTP client")?;

        let base_url = server.unwrap_or_else(|| {
            let port = match target {
                TargetType::Center => DEFAULT_PORT_CENTER,
                TargetType::Server => DEFAULT_PORT_SERVER,
                TargetType::Client => DEFAULT_PORT_CLIENT,
            };
            format!("http://localhost:{}", port)
        });

        Ok(Self {
            client,
            base_url,
            target,
            socket_path: socket,
            token: None,
        })
    }

    /// Create a client that uses `bearer` as a static `Authorization: Bearer` token.
    ///
    /// The caller is responsible for skipping `ensure_auth`. Validation is lazy:
    /// the first real API call returns 401 (unknown token) or 403 (insufficient permission).
    pub fn with_static_bearer(
        target: TargetType,
        server: Option<String>,
        socket: Option<PathBuf>,
        bearer: &str,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0)
            .build()
            .context("Failed to create HTTP client")?;

        let base_url = server.unwrap_or_else(|| {
            let port = match target {
                TargetType::Center => DEFAULT_PORT_CENTER,
                TargetType::Server => DEFAULT_PORT_SERVER,
                TargetType::Client => DEFAULT_PORT_CLIENT,
            };
            format!("http://localhost:{}", port)
        });

        Ok(Self {
            client,
            base_url,
            target,
            socket_path: socket,
            // Wrap in Zeroizing so the credential is wiped on drop.
            token: Some(Zeroizing::new(bearer.to_string())),
        })
    }

    /// Friendly pre-flight auth check. Called before the first API request on the
    /// token-less path (no `--token`, `EDGION_TOKEN`, or `~/.edgion/token`).
    ///
    /// Authentication is token-only: this method never exchanges credentials and
    /// never POSTs to a login endpoint. It only queries the public
    /// `GET /api/v1/auth/status` so it can emit a clear, actionable message when a
    /// token is required but none is configured. When a token *is* present, the
    /// caller uses `with_static_bearer` and skips this entirely — the token is
    /// trusted and the first real request performs lazy 401/403 validation.
    pub async fn ensure_auth(&mut self) -> Result<()> {
        use edgion_resources::auth_dto::{AuthStatus, AuthStatusResponse};

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
        let status: AuthStatusResponse = serde_json::from_value(data)
            .context("/auth/status schema unrecognized -- CLI/controller version mismatch")?;

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

        // Ready and auth is required, but we reached ensure_auth with no token —
        // i.e. none of --token / EDGION_TOKEN / ~/.edgion/token resolved. Emit a
        // clear, actionable message. The CLI no longer performs any credential
        // exchange (token-only auth).
        bail!(
            "this controller requires a bearer token (mode: {}); \
             pass --token <TOKEN>, set the EDGION_TOKEN environment variable, \
             or run `edgion-cli login --token <TOKEN>` to persist one",
            status.mode
        )
    }

    /// Get the target type
    pub fn target(&self) -> TargetType {
        self.target
    }

    /// Get the base URL for connection hints
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Return the active bearer token string for test inspection.
    ///
    /// Only available in test builds so the credential is never exposed in
    /// production code.
    #[cfg(test)]
    pub fn token_for_test(&self) -> Option<&str> {
        self.token.as_ref().map(|t| t.as_str())
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
    pub async fn list_all(&self, kind: &str, limit: Option<u32>, continue_token: Option<&str>) -> Result<Response> {
        let base = match self.target {
            TargetType::Center => self.build_url(&format!("/api/v1/namespaced/{}", kind)),
            TargetType::Server => self.build_url(&format!("/configserver/{}/list", kind)),
            TargetType::Client => self.build_url(&format!("/configclient/{}/list", kind)),
        };
        let url = build_list_url_with_pagination(&base, limit, continue_token)?;
        self.send_request(self.client.get(&url), &url).await
    }

    /// GET request - list resources in a namespace
    /// For center target: uses server-side filtering
    /// For server/client targets: this method should not be called directly,
    /// use list_all() and filter on client side instead
    pub async fn list_namespaced(
        &self,
        kind: &str,
        namespace: &str,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<Response> {
        match self.target {
            TargetType::Center => {
                let base = self.build_url(&format!("/api/v1/namespaced/{}/{}", kind, namespace));
                let url = build_list_url_with_pagination(&base, limit, continue_token)?;
                self.send_request(self.client.get(&url), &url).await
            }
            // For server/client, we fetch all and filter on client side
            // This is handled in get.rs, but we provide a fallback here
            TargetType::Server | TargetType::Client => self.list_all(kind, limit, continue_token).await,
        }
    }

    /// List metadata-only resource keys across all namespaces via the keys
    /// endpoint. Only meaningful when `target == Center`; callers gate this via
    /// the command-level check.
    pub async fn list_all_keys(
        &self,
        kind: &str,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<Response> {
        let base = self.build_url(&format!("/api/v1/keys/namespaced/{}", kind));
        let url = build_list_url_with_pagination(&base, limit, continue_token)?;
        self.send_request(self.client.get(&url), &url).await
    }

    /// List metadata-only resource keys in a specific namespace via the keys
    /// endpoint. Only meaningful when `target == Center`; callers gate this via
    /// the command-level check.
    pub async fn list_namespaced_keys(
        &self,
        kind: &str,
        namespace: &str,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<Response> {
        let base = self.build_url(&format!("/api/v1/keys/namespaced/{}/{}", kind, namespace));
        let url = build_list_url_with_pagination(&base, limit, continue_token)?;
        self.send_request(self.client.get(&url), &url).await
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

    msg.push_str(&format!("\nDetails: {}\n", error));

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
            target: crate::cli::TargetType::Center,
            socket_path: None,
            token: None,
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
    async fn ensure_auth_bails_when_ready_and_auth_required_but_no_token() {
        // Ready + auth_required and we reached ensure_auth (token-less path): must
        // bail with an actionable token message — the CLI never exchanges creds.
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": true,
                "mode": "token",
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
        let msg = format!("{:?}", r.unwrap_err()).to_lowercase();
        assert!(
            msg.contains("token") && (msg.contains("--token") || msg.contains("edgion_token")),
            "err should mention how to supply a token: {msg}"
        );
    }

    #[tokio::test]
    async fn ensure_auth_continues_when_auth_not_required() {
        // Ready but auth_required = false: proceed without a token.
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": false,
                "mode": "token",
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
        assert!(r.is_ok(), "auth_required=false should continue, got {:?}", r);
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
    async fn ensure_auth_bails_on_initializing() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "success": true,
            "data": {
                "auth_required": true,
                "mode": "token",
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
                "mode": "disabled",
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

    /// `with_static_bearer` stores the supplied token and does not read the
    /// file-cached token.
    #[test]
    fn with_static_bearer_sets_token() {
        let c = EdgionClient::with_static_bearer(crate::cli::TargetType::Center, None, None, "edg_testtoken")
            .expect("constructor");
        // The stored token must be the supplied bearer, not None
        assert_eq!(c.token_for_test(), Some("edg_testtoken"));
    }

    /// `with_static_bearer` uses the supplied base_url and the token is set.
    #[test]
    fn with_static_bearer_uses_supplied_url() {
        let c = EdgionClient::with_static_bearer(
            crate::cli::TargetType::Center,
            Some("http://localhost:9999".to_string()),
            None,
            "edg_abc",
        )
        .expect("constructor");
        assert_eq!(c.base_url(), "http://localhost:9999");
        assert_eq!(c.token_for_test(), Some("edg_abc"));
    }
}
