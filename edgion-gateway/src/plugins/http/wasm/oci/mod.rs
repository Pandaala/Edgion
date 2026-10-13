//! Minimal OCI distribution client for `source.oci` Wasm modules.
//!
//! Hand-written rather than an OCI crate: every request must go through the
//! shared outbound policy (`build_outbound_client` + `EgressResolver`), and
//! no available crate exposes injection points for a custom DNS resolver or
//! redirect policy (see the design spec, D2).
//!
//! Pull subset of the OCI Distribution Spec v1: manifest
//! `GET /v2/<repo>/manifests/<ref>` → pick the wasm layer → blob
//! `GET /v2/<repo>/blobs/<digest>`. Private registries answer 401 with a
//! `WWW-Authenticate: Bearer` challenge; the token endpoint converts the
//! dockerconfigjson Basic credentials into a short-lived Bearer token.
//!
//! Redirect policy: manifest and token requests deny redirects; blob
//! requests follow up to 3 (registries routinely 307 blobs to CDN/S3).
//! `EgressResolver` classifies hostname targets at dial time, while a custom
//! redirect policy classifies IP literals (which bypass DNS resolution) on
//! every remote-controlled hop. Reqwest strips `Authorization` on cross-host
//! redirects.

pub mod auth;
pub mod cache;
pub mod manifest;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use sha2::{Digest, Sha256};
use url::{Host, Origin, Url};

use edgion_resources::plugin_configs::{OciImageRef, WasmOciSpec};

use self::auth::{parse_bearer_challenge, select_registry_auth, BearerChallenge, RegistryAuth};
use self::cache::{ArtifactBytes, CacheAccess, CacheKind};
use self::manifest::{
    extract_wasm_from_tar_gz, parse_manifest, select_wasm_layer, OciManifest, ParsedManifest, WasmLayer,
    MANIFEST_ACCEPT,
};
use crate::observe::metrics::record_egress_blocked;
use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
use crate::util::http_body::stream_body_capped;
use crate::util::net::{classify_ip, IpClass};

/// Default per-request timeout for pull requests (manifest, token, blob).
const DEFAULT_PULL_TIMEOUT_SECS: u64 = 60;
/// Default total attempts per request, including the first (same semantics
/// as the url source's `WasmFetchRetry.attempts`).
const DEFAULT_PULL_ATTEMPTS: u32 = 3;
/// Backoff base between retries, doubled per retry. Transport errors and
/// 5xx retry; 4xx never does.
const BACKOFF_MS: u64 = 500;

/// Gateway-local pull tuning (istio-agent `WASM_HTTP_REQUEST_TIMEOUT` /
/// `WASM_HTTP_REQUEST_MAX_RETRIES` parity). Installed once at startup by
/// [`configure_oci_pull`]; the defaults apply until then (and in tests).
static PULL_TIMEOUT_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(DEFAULT_PULL_TIMEOUT_SECS);
static PULL_ATTEMPTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(DEFAULT_PULL_ATTEMPTS);

/// Install the gateway-local OCI pull tuning. Called once at gateway startup;
/// zero values are clamped to 1.
pub fn configure_oci_pull(timeout: Duration, attempts: u32) {
    let timeout_secs = timeout.as_secs().max(1);
    let attempts = attempts.max(1);
    PULL_TIMEOUT_SECS.store(timeout_secs, std::sync::atomic::Ordering::Relaxed);
    PULL_ATTEMPTS.store(attempts, std::sync::atomic::Ordering::Relaxed);
    tracing::info!(timeout_secs, attempts, "Wasm OCI pull tuning configured");
}

fn pull_timeout() -> Duration {
    Duration::from_secs(PULL_TIMEOUT_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

fn pull_attempts() -> u32 {
    PULL_ATTEMPTS.load(std::sync::atomic::Ordering::Relaxed)
}
/// Sanity cap for manifest documents.
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
/// Cap for layer blobs (wasm modules are single-digit MiB in practice).
const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
/// Cap for the token-endpoint JSON response. Bearer tokens are small; this
/// bounds host allocation against a hostile or malfunctioning auth service.
const MAX_TOKEN_BYTES: usize = 64 * 1024;
/// Blob registries may redirect to object storage/CDNs. Match the historical
/// `Limit(3)` contract: allow three redirect responses after the initial URL.
const OCI_BLOB_MAX_REDIRECTS: usize = 3;

/// Result of a successful OCI fetch.
#[derive(Debug)]
pub struct OciFetched {
    /// Final wasm bytecode (post-extraction for the compat variant).
    pub bytes: ArtifactBytes,
    /// Held through the caller's byte-consuming compile stage, including
    /// abandoned blocking work after async cancellation.
    pub(super) pipeline: Arc<OwnedSemaphorePermit>,
    /// The digest (`sha256:<hex>`) that the image reference resolves to at
    /// the TOP level — an image manifest digest, or an index digest when the
    /// reference points at a multi-platform index. This is the value the
    /// `Always` pullPolicy re-check (`resolve_manifest_digest`, which never
    /// descends into an index) compares against, so it must stay the
    /// top-level identity rather than the child image-manifest digest
    /// selected out of an index.
    pub manifest_digest: String,
}

/// Fetch the wasm bytes for `spec`, memory-cache-first. The caller
/// (`vm_manager`) already serializes same-key loads via the `LoadSlot`.
pub async fn fetch_oci_bytes(spec: &WasmOciSpec) -> Result<OciFetched, String> {
    let pipeline = acquire_pipeline().await?;
    let image = OciImageRef::parse(&spec.image).map_err(|e| format!("source.oci.image: {e}"))?;
    let session = RegistrySession::new(&image, spec)?;
    fetch_oci_inner(session, image, pipeline).await
}

async fn acquire_pipeline() -> Result<Arc<OwnedSemaphorePermit>, String> {
    static PIPELINES: OnceLock<Arc<Semaphore>> = OnceLock::new();
    PIPELINES
        .get_or_init(|| Arc::new(Semaphore::new(4)))
        .clone()
        .acquire_owned()
        .await
        .map(Arc::new)
        .map_err(|_| "Wasm OCI acquisition capacity is closed".to_string())
}

/// Every CPU stage captures a lease clone so aborting its async waiter cannot
/// admit another pipeline while this stage still owns full artifact buffers.
pub(super) async fn blocking_stage<T: Send + 'static>(
    pipeline: &Arc<OwnedSemaphorePermit>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let pipeline = pipeline.clone();
    let (result, _pipeline) = tokio::task::spawn_blocking(move || {
        // Keep the permit beside the output too: when the JoinHandle is
        // abandoned, its result bytes must drop before their pipeline lease.
        (work(), pipeline)
    })
    .await
    .map_err(|_| "Wasm OCI artifact processing task failed".to_string())?;
    result
}

async fn fetch_oci_inner(
    mut session: RegistrySession,
    image: OciImageRef,
    pipeline: Arc<OwnedSemaphorePermit>,
) -> Result<OciFetched, String> {
    // Manifest: memory-first for digest refs (zero network on a hit); tag refs
    // resolve over the network.
    let (manifest_bytes, manifest_digest) = fetch_manifest(&mut session, &image, &pipeline).await?;

    // An index costs one more manifest hop; wasm images carry no platform
    // semantics, so take the first entry (Istio behavior). `manifest_digest`
    // stays the TOP-LEVEL digest (image manifest or index) — it is the
    // identity the `Always` re-check (`resolve_manifest_digest`, which never
    // descends an index) compares against, so it must not be overwritten
    // with the child image-manifest digest here.
    let parsed = blocking_stage(&pipeline, move || parse_manifest(&manifest_bytes)).await?;
    let image_manifest: OciManifest = match parsed {
        ParsedManifest::Image(m) => m,
        ParsedManifest::Index(entries) => {
            let digest = entries[0].digest.clone();
            let bytes = fetch_manifest_by_digest(&mut session, &digest, &pipeline).await?;
            match blocking_stage(&pipeline, move || parse_manifest(&bytes)).await? {
                ParsedManifest::Image(m) => m,
                ParsedManifest::Index(_) => return Err("nested image index is not supported".to_string()),
            }
        }
    };

    let (descriptor, compat) = match select_wasm_layer(&image_manifest)? {
        WasmLayer::Raw(d) => (d.clone(), false),
        WasmLayer::TarGz(d) => (d.clone(), true),
    };
    let blob = fetch_blob(&mut session, &descriptor.digest, &pipeline).await?;
    let bytes = if compat {
        blocking_stage(&pipeline, move || {
            extract_wasm_from_tar_gz(&blob).map(ArtifactBytes::from)
        })
        .await?
    } else {
        blob
    };
    Ok(OciFetched {
        bytes,
        manifest_digest,
        pipeline,
    })
}

/// Resolve the current manifest digest for `spec` (the `Always` pullPolicy
/// re-check). One small manifest request; no blob traffic.
pub async fn resolve_manifest_digest(spec: &WasmOciSpec) -> Result<String, String> {
    let image = OciImageRef::parse(&spec.image).map_err(|e| format!("source.oci.image: {e}"))?;
    if let Some(d) = &image.digest {
        return Ok(d.clone());
    }
    let pipeline = acquire_pipeline().await?;
    let mut session = RegistrySession::new(&image, spec)?;
    let (_, digest) = fetch_manifest(&mut session, &image, &pipeline).await?;
    Ok(digest)
}

// ---------------------------------------------------------------------------
// Registry session
// ---------------------------------------------------------------------------

/// Retryability classification for one request attempt.
enum GetError {
    /// Transport error or 5xx: retried up to `ATTEMPTS`.
    Retryable(String),
    /// 4xx or a protocol dead end: never retried.
    Definitive(String),
}

struct RegistrySession {
    cache: CacheAccess,
    /// `https://host[:port]` (`http://` for loopback registries).
    base: String,
    repo: String,
    auth: Option<RegistryAuth>,
    bearer: Option<String>,
    registry_origin: Origin,
    registry_uses_tls: bool,
    egress_policy: EgressPolicy,
    client_deny: reqwest::Client,
    client_blob: reqwest::Client,
}

impl RegistrySession {
    fn new(image: &OciImageRef, spec: &WasmOciSpec) -> Result<Self, String> {
        use crate::link_sys::http_client::OutboundRedirect;
        // Fail the load rather than silently pulling anonymously when the
        // controller has not (yet) resolved the referenced Secret.
        if spec.pull_secret_ref.is_some() && spec.resolved_pull_secret.is_none() {
            return Err("wasm oci pull secret not resolved".to_string());
        }
        let api_host = image.api_host();
        let auth = match spec.resolved_pull_secret.as_deref() {
            Some(json) => select_registry_auth(json, &api_host)?,
            None => None,
        };
        let base = registry_base(&api_host);
        let registry_url = Url::parse(&base).map_err(|_| "wasm oci registry URL is invalid".to_string())?;
        let registry_uses_tls = registry_url.scheme() == "https";
        let registry_origin = registry_url.origin();
        let egress_policy = EgressPolicy {
            block_private: default_block_private(),
        };
        Ok(RegistrySession {
            cache: CacheAccess::active(),
            base,
            repo: image.repository.clone(),
            auth,
            bearer: None,
            registry_origin: registry_origin.clone(),
            registry_uses_tls,
            egress_policy,
            client_deny: outbound_client(OutboundRedirect::Deny, pull_timeout(), egress_policy)?,
            client_blob: outbound_client(
                OutboundRedirect::Custom(blob_redirect_policy(registry_origin, egress_policy)),
                pull_timeout(),
                egress_policy,
            )?,
        })
    }

    /// GET with credentials, the 401 Bearer-challenge flow, and the fixed
    /// retry policy. Each logical attempt competes for the global Wasm fetch
    /// semaphore independently; retry backoff never occupies a permit.
    async fn get(&mut self, blob: bool, url: &str, accept: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        self.get_with_permits(super::vm_manager::url_fetch_permits(), blob, url, accept, max_bytes)
            .await
    }

    async fn get_with_permits(
        &mut self,
        permits: &tokio::sync::Semaphore,
        blob: bool,
        url: &str,
        accept: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let mut last_error = String::new();
        let attempts = pull_attempts();
        for attempt in 0..attempts {
            if attempt > 0 {
                let delay = BACKOFF_MS.saturating_mul(1u64 << (attempt - 1).min(10));
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let permit = permits
                .acquire()
                .await
                .map_err(|_| "wasm fetch semaphore closed".to_string())?;
            // One logical attempt includes the initial registry request and,
            // when challenged, token fetch + Bearer resend. None of those
            // helpers acquires recursively.
            let result = self.get_authed(blob, url, accept, max_bytes).await;
            drop(permit);
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(GetError::Definitive(e)) => return Err(e),
                Err(GetError::Retryable(e)) => last_error = e,
            }
        }
        Err(last_error)
    }

    async fn get_authed(&mut self, blob: bool, url: &str, accept: &str, max_bytes: usize) -> Result<Vec<u8>, GetError> {
        let mut resp = self.send(blob, url, accept).await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED && self.bearer.is_none() {
            let challenge = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_bearer_challenge);
            // Without a Bearer challenge the 401 is definitive (Basic was
            // wrong or missing) and falls through to the status check below.
            if let Some(c) = challenge {
                self.bearer = Some(self.fetch_token(&c).await?);
                resp = self.send(blob, url, accept).await?;
            }
        }
        let status = resp.status();
        if !status.is_success() {
            let msg = format!("GET {url}: status {status}");
            return Err(if status.is_client_error() {
                GetError::Definitive(msg)
            } else {
                GetError::Retryable(msg)
            });
        }
        // Reject an honestly-oversized Content-Length before opening the body
        // stream (fast path, mirrors `outbound_call`). The streaming counter
        // below is the load-bearing guard for absent/false/chunked lengths.
        if resp.content_length().is_some_and(|cl| cl > max_bytes as u64) {
            return Err(GetError::Definitive(format!(
                "GET {url}: body exceeds {max_bytes} bytes"
            )));
        }
        // Stream the body with a running cap: peak host allocation is bounded
        // to `max_bytes` plus one chunk, so a response with no/false/chunked
        // Content-Length cannot make reqwest buffer an arbitrarily large body
        // before the size check fires.
        let (body, truncated) = stream_body_capped(resp, max_bytes)
            .await
            .map_err(|e| GetError::Retryable(format!("read body of {url}: {}", e.without_url())))?;
        if truncated {
            return Err(GetError::Definitive(format!(
                "GET {url}: body exceeds {max_bytes} bytes"
            )));
        }
        Ok(body.into())
    }

    async fn send(&self, blob: bool, url: &str, accept: &str) -> Result<reqwest::Response, GetError> {
        let client = if blob { &self.client_blob } else { &self.client_deny };
        let mut req = client.get(url).header(reqwest::header::ACCEPT, accept);
        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        } else if let Some(a) = &self.auth {
            req = req.basic_auth(&a.username, Some(&a.password));
        }
        req.send().await.map_err(|e| {
            if e.is_redirect() {
                GetError::Definitive("OCI redirect blocked by policy".to_string())
            } else {
                GetError::Retryable(format!("GET {url}: {}", e.without_url()))
            }
        })
    }

    async fn fetch_token(&self, challenge: &BearerChallenge) -> Result<String, GetError> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            #[serde(default)]
            token: Option<String>,
            #[serde(default)]
            access_token: Option<String>,
        }
        let scope = challenge
            .scope
            .clone()
            .unwrap_or_else(|| format!("repository:{}:pull", self.repo));
        let mut query: Vec<(&str, &str)> = vec![("scope", &scope)];
        if let Some(service) = &challenge.service {
            query.push(("service", service));
        }
        let realm = Url::parse(&challenge.realm)
            .map_err(|_| GetError::Definitive("token endpoint URL is invalid".to_string()))?;
        if token_realm_downgrades_tls(self.registry_uses_tls, &realm) {
            return Err(GetError::Definitive(
                "token endpoint blocked by transport policy".to_string(),
            ));
        }
        guard_remote_target_and_record(&realm, &self.registry_origin, self.egress_policy)
            .map_err(|_| GetError::Definitive("token endpoint blocked by egress policy".to_string()))?;
        let mut req = self.client_deny.get(realm).query(&query);
        if let Some(a) = &self.auth {
            req = req.basic_auth(&a.username, Some(&a.password));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| GetError::Retryable(format!("token endpoint: {}", e.without_url())))?;
        let status = resp.status();
        if !status.is_success() {
            // Classify consistently with registry GET responses: 4xx (bad or
            // missing credentials) is a definitive auth failure, but a 5xx from
            // a flaky authorization service is transient and must honor the
            // configured retry policy. The message deliberately omits the
            // response body and the token.
            let msg = format!("token endpoint: status {status}");
            return Err(if status.is_client_error() {
                GetError::Definitive(msg)
            } else {
                GetError::Retryable(msg)
            });
        }
        // Bound host allocation on the token body too: fast-reject an honest
        // oversized Content-Length, then stream with a small cap rather than
        // `resp.json()` (which would buffer an unbounded body).
        if resp.content_length().is_some_and(|cl| cl > MAX_TOKEN_BYTES as u64) {
            return Err(GetError::Definitive(format!(
                "token endpoint: response exceeds {MAX_TOKEN_BYTES} bytes"
            )));
        }
        let (body, truncated) = stream_body_capped(resp, MAX_TOKEN_BYTES)
            .await
            .map_err(|e| GetError::Retryable(format!("token response: {}", e.without_url())))?;
        if truncated {
            return Err(GetError::Definitive(format!(
                "token endpoint: response exceeds {MAX_TOKEN_BYTES} bytes"
            )));
        }
        let parsed: TokenResponse =
            serde_json::from_slice(&body).map_err(|e| GetError::Retryable(format!("token response: {e}")))?;
        parsed
            .token
            .or(parsed.access_token)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| GetError::Definitive("token response contains no token".to_string()))
    }
}

fn token_realm_downgrades_tls(registry_uses_tls: bool, realm: &Url) -> bool {
    registry_uses_tls && realm.scheme() == "http"
}

/// `https://` everywhere except loopback (`localhost` / 127.0.0.0/8 / `::1`),
/// which uses plain HTTP so local test registries work without a TLS knob.
/// Non-loopback registries must present a publicly-verifiable certificate in
/// v1 (no custom-CA field yet).
fn registry_base(registry: &str) -> String {
    let host = registry.rsplit_once(':').map(|(h, _)| h).unwrap_or(registry);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let is_loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if is_loopback {
        format!("http://{registry}")
    } else {
        format!("https://{registry}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteTargetError {
    Invalid,
    Blocked(&'static str),
}

/// Guard only targets introduced by a remote OCI response. The operator's
/// configured registry origin is an explicit trust decision and remains
/// allowed even when it is a local development registry. Cross-origin
/// hostnames continue through the shared `EgressResolver`; IP literals are
/// classified here because reqwest does not send them through DNS resolution.
fn guard_remote_target(target: &Url, registry_origin: &Origin, policy: EgressPolicy) -> Result<(), RemoteTargetError> {
    if &target.origin() == registry_origin {
        return Ok(());
    }
    let ip = match target.host() {
        Some(Host::Domain(_)) => return Ok(()),
        Some(Host::Ipv4(ip)) => std::net::IpAddr::V4(ip),
        Some(Host::Ipv6(ip)) => std::net::IpAddr::V6(ip),
        None => return Err(RemoteTargetError::Invalid),
    };
    let reason = match classify_ip(ip) {
        IpClass::Loopback => "loopback",
        IpClass::LinkLocal => "link_local",
        IpClass::Unspecified => "unspecified",
        IpClass::Private | IpClass::UniqueLocal if policy.block_private => "private",
        IpClass::Private | IpClass::UniqueLocal | IpClass::Global => return Ok(()),
    };
    Err(RemoteTargetError::Blocked(reason))
}

fn guard_remote_target_and_record(
    target: &Url,
    registry_origin: &Origin,
    policy: EgressPolicy,
) -> Result<(), RemoteTargetError> {
    guard_remote_target(target, registry_origin, policy).inspect_err(|&error| {
        if let RemoteTargetError::Blocked(reason) = error {
            record_egress_blocked(reason);
        }
    })
}

fn blob_redirect_policy(registry_origin: Origin, policy: EgressPolicy) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        // `previous` contains the initial URL, so the fourth redirect is the
        // first attempt whose history length exceeds the three-hop limit.
        if attempt.previous().len() > OCI_BLOB_MAX_REDIRECTS {
            return attempt.error("too many OCI blob redirects");
        }
        if redirect_downgrades_tls(attempt.previous(), attempt.url()) {
            return attempt.error("OCI blob redirect blocked by transport policy");
        }
        match guard_remote_target_and_record(attempt.url(), &registry_origin, policy) {
            Ok(()) => attempt.follow(),
            Err(_) => attempt.error("OCI blob redirect blocked by egress policy"),
        }
    })
}

fn redirect_downgrades_tls(previous: &[Url], target: &Url) -> bool {
    previous
        .last()
        .is_some_and(|source| source.scheme() == "https" && target.scheme() == "http")
}

fn outbound_client(
    redirect: crate::link_sys::http_client::OutboundRedirect,
    total_timeout: Duration,
    egress_policy: EgressPolicy,
) -> Result<reqwest::Client, String> {
    use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig};
    use edgion_common::dns::ResolverSpec;

    let egress: Arc<dyn reqwest::dns::Resolve> = Arc::new(EgressResolver::new(
        ResolverSpec::System,
        egress_policy,
        egress_dns_cache_ttl(),
    ));
    build_outbound_client(OutboundClientConfig {
        pool_max_idle_per_host: Some(4),
        connect_timeout: Some(Duration::from_secs(5)),
        total_timeout: Some(total_timeout),
        redirect,
        disable_proxy: true,
        tls_context: Some("WasmOciFetch"),
        dns_resolver: Some(egress),
        ..Default::default()
    })
    .map_err(|e| format!("wasm oci client: {e}"))
}

/// Verify `bytes` against an OCI digest string (`sha256:<64 hex>`).
fn verify_digest(bytes: &[u8], digest: &str) -> Result<(), String> {
    let hex_part = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unsupported digest algorithm '{digest}' (only sha256)"))?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(hex_part) {
        Ok(())
    } else {
        Err(format!("digest mismatch: expected {digest}, got sha256:{actual}"))
    }
}

async fn fetch_manifest(
    session: &mut RegistrySession,
    image: &OciImageRef,
    pipeline: &Arc<OwnedSemaphorePermit>,
) -> Result<(ArtifactBytes, String), String> {
    if let Some(digest) = &image.digest {
        let bytes = fetch_manifest_by_digest(session, digest, pipeline).await?;
        return Ok((bytes, digest.clone()));
    }
    let tag = image.tag.as_deref().unwrap_or("latest");
    let url = format!("{}/v2/{}/manifests/{}", session.base, session.repo, tag);
    let bytes = session.get(false, &url, MANIFEST_ACCEPT, MAX_MANIFEST_BYTES).await?;
    let (bytes, hex_part) = blocking_stage(pipeline, move || {
        let hex_part = hex::encode(Sha256::digest(&bytes));
        Ok((bytes, hex_part))
    })
    .await?;
    let bytes = session.cache.store_verified(CacheKind::Manifest, &hex_part, bytes);
    Ok((bytes, format!("sha256:{hex_part}")))
}

async fn fetch_manifest_by_digest(
    session: &mut RegistrySession,
    digest: &str,
    pipeline: &Arc<OwnedSemaphorePermit>,
) -> Result<ArtifactBytes, String> {
    let url = format!("{}/v2/{}/manifests/{}", session.base, session.repo, digest);
    cached_verified_fetch(session.cache.clone(), CacheKind::Manifest, digest, pipeline, || {
        session.get(false, &url, MANIFEST_ACCEPT, MAX_MANIFEST_BYTES)
    })
    .await
}

async fn fetch_blob(
    session: &mut RegistrySession,
    digest: &str,
    pipeline: &Arc<OwnedSemaphorePermit>,
) -> Result<ArtifactBytes, String> {
    let url = format!("{}/v2/{}/blobs/{}", session.base, session.repo, digest);
    cached_verified_fetch(session.cache.clone(), CacheKind::Blob, digest, pipeline, || {
        session.get(true, &url, "application/octet-stream", MAX_BLOB_BYTES)
    })
    .await
}

/// Both immutable OCI kinds share the same claim, validation and admission
/// boundary. The loader owns transport policy; tests inject bounded memory data.
async fn cached_verified_fetch<F, Fut>(
    cache: CacheAccess,
    kind: CacheKind,
    digest: &str,
    pipeline: &Arc<OwnedSemaphorePermit>,
    load: F,
) -> Result<ArtifactBytes, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let hex_part = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unsupported digest algorithm '{digest}' (only sha256)"))?;
    if let Some(bytes) = cache.load(kind, hex_part) {
        return Ok(bytes);
    }
    let _claim = cache.claim(kind, hex_part).await;
    if let Some(bytes) = cache.load(kind, hex_part) {
        return Ok(bytes);
    }
    let bytes = load().await?;
    let expected = digest.to_string();
    let bytes = blocking_stage(pipeline, move || {
        verify_digest(&bytes, &expected)?;
        Ok(bytes)
    })
    .await?;
    Ok(cache.store_verified(kind, hex_part, bytes))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/wasm/oci/mod_test/memory_pipeline_tests.rs"]
mod memory_pipeline_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/wasm/oci/mod_test.rs"]
mod tests;
