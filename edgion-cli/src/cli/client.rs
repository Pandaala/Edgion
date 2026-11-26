use anyhow::{bail, Context, Result};
use edgion_common::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use edgion_common::link_sys_core::tls::build_custom_ca_http_rustls_config;
use reqwest::{Client, Response};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zeroize::Zeroizing;

use super::TargetType;

#[derive(Debug, Clone, Copy)]
pub(super) enum AuthProbeError {
    Http(u16),
    InvalidResponse,
    Initializing,
    TokenRequired,
}

impl std::fmt::Display for AuthProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(status) => write!(f, "/auth/status returned HTTP {status}"),
            Self::InvalidResponse => f.write_str("invalid Controller authentication response"),
            Self::Initializing => f.write_str("Controller authentication is initializing, retry later"),
            Self::TokenRequired => f.write_str(
                "Controller requires a bearer token: pass --token, set EDGION_TOKEN, or use edgion-cli login --token",
            ),
        }
    }
}

impl std::error::Error for AuthProbeError {}

/// Append `limit` and `continue` query params to a URL.
///
/// Uses `query_pairs_mut` so values are properly URL-encoded (continue tokens are
/// base64-url-safe per K8s spec, but be defensive).
fn build_list_url_with_pagination(base: &str, limit: Option<u32>, continue_token: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(base)?;
    let limit = if url.path().starts_with("/api/v1/") {
        let value = limit.unwrap_or(10);
        if !(1..=100).contains(&value) {
            bail!("Source page size must be between 1 and 100");
        }
        Some(value)
    } else {
        limit
    };
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

/// Which CA certificates verify an `https://` server, and an `https://` proxy
/// (reqwest uses the same TLS configuration for both).
#[derive(Clone, Default)]
pub enum ServerTrust {
    /// The OS CA bundle (`edgion_common::system_trust`).
    #[default]
    System,
    /// Only the certificates in `--ca-cert` / `EDGION_CA_CERT`; replaces System.
    CaFile {
        path: PathBuf,
        tls: Box<rustls::ClientConfig>,
    },
}

impl std::fmt::Debug for ServerTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => f.write_str("System"),
            Self::CaFile { path, .. } => f.debug_struct("CaFile").field("path", path).finish_non_exhaustive(),
        }
    }
}

impl ServerTrust {
    /// Load `--ca-cert` (or `EDGION_CA_CERT`) when given; an empty path means
    /// unset. The file is read with a bound and validated up front, so a bad file
    /// fails the command instead of surfacing later as an opaque handshake error.
    /// It is read even for an `http://` server, so a stale setting is reported.
    /// Pipes are accepted, so `--ca-cert <(...)` works; as with curl, a FIFO
    /// with no writer blocks.
    pub fn from_ca_cert(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path.filter(|path| !path.as_os_str().is_empty()) else {
            return Ok(Self::System);
        };
        let limit = edgion_common::system_trust::MAX_CA_FILE_BYTES;
        let mut pem = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(limit + 1).read_to_end(&mut pem))
            .with_context(|| format!("failed to read CA certificate file {}", path.display()))?;
        if pem.len() as u64 > limit {
            bail!("CA certificate file {} exceeds {} bytes", path.display(), limit);
        }
        let tls = build_custom_ca_http_rustls_config(&pem)
            .with_context(|| format!("invalid CA certificate file {}", path.display()))?;
        Ok(Self::CaFile {
            path: path.to_path_buf(),
            tls: Box::new(tls),
        })
    }

    /// Build a CLI HTTP client from `cfg` with this trust applied. `System`
    /// keeps the factory's System-root default.
    pub(crate) fn build_client(&self, cfg: OutboundClientConfig) -> Result<Client> {
        let preconfigured_tls = match self {
            Self::System => None,
            Self::CaFile { tls, .. } => Some(rustls::ClientConfig::clone(tls)),
        };
        build_outbound_client(OutboundClientConfig {
            preconfigured_tls,
            ..cfg
        })
    }
}

/// `login` uses the trust only for its best-effort probe, which never blocks
/// saving the token: an unusable CA file skips the probe with a warning.
pub(crate) fn login_probe_trust(path: Option<&Path>) -> Option<ServerTrust> {
    match ServerTrust::from_ca_cert(path) {
        Ok(trust) => Some(trust),
        Err(error) => {
            eprintln!("warning: {error:#}; skipping the server probe (the token is still saved).");
            None
        }
    }
}

/// Render an error with its cause chain. reqwest's own message ("error sending
/// request") hides the cause, such as a TLS verification failure. Callers pass
/// an error whose URL was already removed with `without_url`.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

/// Default server ports for different targets
const DEFAULT_PORT_CENTER: u16 = 12101; // Controller ConfCenter CRUD API
const DEFAULT_PORT_SERVER: u16 = 12101; // Controller ConfigServer cache
const DEFAULT_PORT_CLIENT: u16 = 12001; // Gateway admin

/// Build the shared outbound client used by every `EdgionClient` constructor.
///
/// Routed through the canonical `build_outbound_client` factory (design rule
/// D.1) so transport policy is not re-derived per call site. Two knobs are
/// deliberately *not* the server-side defaults:
///
/// - `disable_proxy: false` — unlike gateway/controller egress, this is an
///   operator-run CLI; honoring `HTTP(S)_PROXY` is required to reach a
///   controller from behind a corporate proxy.
/// - `pool_max_idle_per_host: Some(2)` — multi-request subcommands (`get --all`
///   pagination, multi-document `apply`, which issues a probe GET plus a
///   PUT/POST per document) would otherwise pay a fresh TCP (and TLS) handshake
///   per request. A CLI process talks to exactly one host, so a small bound is
///   enough to keep the connection warm across a serial command.
fn build_cli_client(trust: &ServerTrust) -> Result<Client> {
    trust
        .build_client(OutboundClientConfig {
            total_timeout: Some(Duration::from_secs(10)),
            connect_timeout: Some(Duration::from_secs(5)),
            pool_max_idle_per_host: Some(2),
            // The admin API never redirects; following one would silently carry the
            // bearer token across an https -> http downgrade on the same host.
            redirect: OutboundRedirect::Deny,
            disable_proxy: false,
            ..Default::default()
        })
        .context("Failed to create HTTP client")
}

/// Without `--ca-cert`, an `https://` server is verified against the OS CA
/// bundle. Without one, every handshake fails with an opaque `UnknownIssuer`,
/// so say why up front.
fn warn_if_system_trust_unavailable(base_url: &str, trust: &ServerTrust) {
    if !matches!(trust, ServerTrust::System) || !base_url.to_ascii_lowercase().starts_with("https://") {
        return;
    }
    if let edgion_common::system_trust::SystemTrustSource::Unavailable(reason) =
        edgion_common::system_trust::system_trust().source()
    {
        eprintln!("Warning: HTTPS server certificates cannot be verified (System CA trust unavailable: {reason})");
    }
}

/// EdgionClient for interacting with Controller/Gateway APIs
pub struct EdgionClient {
    client: Client,
    base_url: String,
    target: TargetType,
    token: Option<Zeroizing<String>>,
}

impl EdgionClient {
    /// Create a new client with no bearer token.
    ///
    /// Authentication is token-only: when no token is resolved (`--token`,
    /// `EDGION_TOKEN`, or `~/.edgion/token`), this token-less client is created so
    /// `ensure_auth` can print a friendly "no token configured" message. With a
    /// resolved token the caller uses `with_static_bearer` instead.
    pub fn new(
        target: TargetType,
        server: Option<String>,
        _socket: Option<PathBuf>,
        trust: &ServerTrust,
    ) -> Result<Self> {
        let client = build_cli_client(trust)?;

        let base_url = server.unwrap_or_else(|| {
            let port = match target {
                TargetType::Center => DEFAULT_PORT_CENTER,
                TargetType::Server => DEFAULT_PORT_SERVER,
                TargetType::Client => DEFAULT_PORT_CLIENT,
            };
            format!("http://localhost:{}", port)
        });
        warn_if_system_trust_unavailable(&base_url, trust);

        Ok(Self {
            client,
            base_url,
            target,
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
        _socket: Option<PathBuf>,
        trust: &ServerTrust,
        bearer: &str,
    ) -> Result<Self> {
        let client = build_cli_client(trust)?;

        let base_url = server.unwrap_or_else(|| {
            let port = match target {
                TargetType::Center => DEFAULT_PORT_CENTER,
                TargetType::Server => DEFAULT_PORT_SERVER,
                TargetType::Client => DEFAULT_PORT_CLIENT,
            };
            format!("http://localhost:{}", port)
        });
        warn_if_system_trust_unavailable(&base_url, trust);

        Ok(Self {
            client,
            base_url,
            target,
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
            .map_err(reqwest::Error::without_url)
            .with_context(|| {
                format!(
                    "cannot reach controller at {} (network/TLS error)",
                    diagnostic_origin(&self.base_url)
                )
            })?;

        let status_code = resp.status();
        if !status_code.is_success() {
            return Err(AuthProbeError::Http(status_code.as_u16()).into());
        }

        #[derive(serde::Deserialize)]
        struct Envelope {
            data: AuthStatusResponse,
        }
        let status = resp
            .json::<Envelope>()
            .await
            .map_err(|_| AuthProbeError::InvalidResponse)?
            .data;

        match status.status {
            AuthStatus::Disabled => return Ok(()),
            AuthStatus::Initializing => {
                return Err(AuthProbeError::Initializing.into());
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
        Err(AuthProbeError::TokenRequired.into())
    }

    /// Get the target type
    pub fn target(&self) -> TargetType {
        self.target
    }

    /// Get the transport URL. Use `diagnostic_origin` before displaying it.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Build full URL for API endpoint
    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authenticate_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref token) = self.token {
            request = request.bearer_auth(token.as_str());
        }
        request
    }

    /// Send HTTP request with detailed error handling
    async fn send_request(&self, request: reqwest::RequestBuilder, url: &str) -> Result<Response> {
        self.authenticate_request(request).send().await.map_err(|e| {
            let error_msg = format_request_error(url, e, self.target);
            anyhow::anyhow!("{}", error_msg)
        })
    }

    pub(crate) async fn server_info(&self) -> Result<Value> {
        let url = self.build_url("/api/v1/server-info");
        let response = self.send_request(self.client.get(&url), &url).await?;
        let status = response.status();
        if !status.is_success() {
            bail!("Controller server-info request failed with HTTP {status}");
        }
        let envelope: Value = response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Failed to read Controller server-info JSON")?;
        let data = &envelope["data"];
        if envelope["success"] != true
            || !data["server_id"].is_string()
            || !data["ready"].is_boolean()
            || !data["mode"].is_string()
        {
            bail!("Controller did not return valid server information");
        }
        Ok(envelope)
    }

    pub(crate) async fn diagnostics(&self, report: super::DiagnosticReport) -> Result<Value> {
        let path = match report {
            super::DiagnosticReport::Conflicts => "/api/v1/diagnostics/conf-conflicts",
            super::DiagnosticReport::UnknownFields => "/api/v1/diagnostics/unknown-fields",
        };
        let url = self.build_url(path);
        let response = self.send_request(self.client.get(&url), &url).await?;
        let status = response.status();
        if status == reqwest::StatusCode::CONFLICT {
            return Err(diagnostics_conflict_error(response).await);
        }
        if !status.is_success() {
            bail!("Controller diagnostics request failed with HTTP {status}");
        }
        let envelope: Value = response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Failed to read Controller diagnostics JSON")?;
        if envelope["success"] != true || !report.matches_data(&envelope["data"]) {
            bail!("Controller did not return a successful {report:?} diagnostics report");
        }
        Ok(envelope)
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

    /// Submit one document to the read-only Controller validation endpoint.
    pub(crate) async fn validate(&self, kind: edgion_resources::ResourceKind, body: String) -> Result<&'static str> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            success: bool,
            data: Verdict,
        }

        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Verdict {
            ok: bool,
            unknown_field_check: Coverage,
            unknown_fields: Vec<String>,
            schema_errors: Vec<String>,
            validation_errors: Vec<String>,
        }

        #[derive(serde::Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Coverage {
            Performed,
            Unsupported,
        }

        let url = self.build_url(&format!("/api/v1/validate/{}", kind.as_str()));
        let request = self
            .client
            .post(&url)
            .header("Content-Type", "application/yaml")
            .body(body);
        let response = self.send_request(request, &url).await?;
        if !response.status().is_success() {
            bail!("Controller validation returned HTTP {}", response.status());
        }
        let envelope: Envelope = response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Invalid Controller validation response")?;
        if !envelope.success {
            bail!("Controller validation request failed");
        }
        let verdict = envelope.data;
        let mut diagnostics = Vec::new();
        for (label, entries) in [
            ("schema error", verdict.schema_errors),
            ("unknown field", verdict.unknown_fields),
            ("validation error", verdict.validation_errors),
        ] {
            diagnostics.extend(entries.into_iter().map(|entry| format!("{label}: {entry}")));
        }
        if !diagnostics.is_empty() {
            bail!("{}", diagnostics.join("\n"));
        }
        if !verdict.ok {
            bail!("Controller rejected the document without diagnostics");
        }
        Ok(match verdict.unknown_field_check {
            Coverage::Performed => "performed",
            Coverage::Unsupported => "unsupported",
        })
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
}

/// Display the advertised address without forwarding credentials or choosing
/// a transport for an operator who may be connecting through a TLS proxy.
async fn diagnostics_conflict_error(mut response: Response) -> anyhow::Error {
    let fallback = anyhow::anyhow!("Controller diagnostics request failed with HTTP 409 Conflict");
    let mut body = Vec::new();
    const MAX_HINT_BYTES: usize = 4096;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_HINT_BYTES - body.len() => body.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => return fallback,
        }
    }
    let Ok(hint) = serde_json::from_slice::<Value>(&body) else {
        return fallback;
    };
    if hint["error"] != "not leader" {
        return fallback;
    }
    if let Some(address) = hint["leader"].as_str().and_then(leader_hint_address) {
        return anyhow::anyhow!("{fallback}: not leader; retry on the leader at {address}");
    }
    anyhow::anyhow!("{fallback}: not leader; retry on the active Controller")
}

fn leader_hint_address(raw: &str) -> Option<String> {
    if raw.contains("://") {
        return http_origin(raw);
    }
    let (host, port) = raw.rsplit_once(':')?;
    if !host.bytes().all(|c| c.is_ascii_alphanumeric() || b".-:[]".contains(&c)) {
        return None;
    }
    let host = url::Host::parse(host).ok()?;
    let port = port.parse::<u16>().ok()?;
    Some(format!("{host}:{port}"))
}

fn http_origin(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.origin().ascii_serialization())
}

/// Project an endpoint to its HTTP(S) origin without credentials or request data.
pub(crate) fn diagnostic_origin(raw: &str) -> String {
    http_origin(raw).unwrap_or_else(|| "<invalid endpoint>".to_string())
}

/// Format network error with detailed diagnostics.
fn format_request_error(url: &str, error: reqwest::Error, target: TargetType) -> String {
    let url = diagnostic_origin(url);
    let error = error.without_url();
    let details = error_chain(&error);
    let mut msg = format!("Request to {} failed\n", url);

    // Determine error type and provide specific hints
    let error_string = details.to_lowercase();
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
    } else if error_string.contains("invalid peer certificate") {
        Some(
            "The server's (or https:// proxy's) TLS certificate could not be verified - if a private CA \
             issued it, pass that CA with --ca-cert"
                .to_string(),
        )
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

    msg.push_str(&format!("\nDetails: {}\n", details));

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
#[path = "../../tests/unit/cli/client_test/ensure_auth_tests.rs"]
mod ensure_auth_tests;

#[cfg(test)]
#[path = "../../tests/unit/cli/client_test/source_paging_tests.rs"]
mod source_paging_tests;

#[cfg(test)]
#[path = "../../tests/unit/cli/client_test/server_trust_tests.rs"]
mod server_trust_tests;
