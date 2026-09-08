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
use crate::plugins::http::common::outbound::call::stream_body_capped;
use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
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
            .map_err(|e| GetError::Retryable(format!("read body of {url}: {e}")))?;
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
                GetError::Retryable(format!("GET {url}: {e}"))
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
            .map_err(|e| GetError::Retryable(format!("token endpoint: {e}")))?;
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
            .map_err(|e| GetError::Retryable(format!("token response: {e}")))?;
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
    guard_remote_target(target, registry_origin, policy).map_err(|error| {
        if let RemoteTargetError::Blocked(reason) = error {
            record_egress_blocked(reason);
        }
        error
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
#[path = "memory_pipeline_tests.rs"]
mod memory_pipeline_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const WASM: &[u8] = b"\0asm\x01\x00\x00\x00";

    fn digest_of(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    fn image_manifest(layer: &[u8], media_type: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{"mediaType": media_type, "digest": digest_of(layer), "size": layer.len()}]
        }))
        .unwrap()
    }

    fn spec_for(server: &MockServer, image_suffix: &str) -> WasmOciSpec {
        let host = server.uri().trim_start_matches("http://").to_string();
        serde_json::from_value(serde_json::json!({ "image": format!("{host}/{image_suffix}") })).unwrap()
    }

    async fn mount_image(server: &MockServer, reference: &str, manifest: &[u8], blob: &[u8]) {
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/manifests/{reference}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.to_vec()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(blob))))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob.to_vec()))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn anonymous_pull_oci_variant() {
        let server = MockServer::start().await;
        let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
        mount_image(&server, "1.0.0", &manifest, WASM).await;
        let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:1.0.0")).await.unwrap();
        assert_eq!(fetched.bytes.as_ref(), WASM);
        assert_eq!(fetched.manifest_digest, digest_of(&manifest));
    }

    #[tokio::test]
    async fn compat_variant_extracts_plugin_wasm() {
        let server = MockServer::start().await;
        let layer = {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            let enc = GzEncoder::new(Vec::new(), Compression::default());
            let mut b = tar::Builder::new(enc);
            let mut h = tar::Header::new_gnu();
            h.set_size(WASM.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "plugin.wasm", WASM).unwrap();
            b.into_inner().unwrap().finish().unwrap()
        };
        let manifest = image_manifest(&layer, "application/vnd.docker.image.rootfs.diff.tar.gzip");
        mount_image(&server, "compat", &manifest, &layer).await;
        let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:compat")).await.unwrap();
        assert_eq!(fetched.bytes.as_ref(), WASM);
    }

    #[tokio::test]
    async fn private_pull_via_bearer_token_flow() {
        let server = MockServer::start().await;
        let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
        let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
        // Catch-all 401 with the challenge (lower priority).
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .with_priority(5)
            .mount(&server)
            .await;
        // Token endpoint requires the Basic credentials from the pull secret.
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("alice:s3cret")
        );
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("service", "test-registry"))
            .and(header("authorization", basic.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "test-token"})))
            .mount(&server)
            .await;
        // Bearer-authed requests succeed (higher priority than the 401).
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.clone()))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
            .mount(&server)
            .await;

        let host = server.uri().trim_start_matches("http://").to_string();
        let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
        let mut spec = spec_for(&server, "test/wasm:1.0.0");
        spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
        let fetched = fetch_oci_bytes(&spec).await.unwrap();
        assert_eq!(fetched.bytes.as_ref(), WASM);
    }

    #[test]
    fn remote_target_policy_covers_origins_and_ip_classes() {
        let registry = Url::parse("http://127.0.0.1:5000").unwrap().origin();
        let permissive = EgressPolicy { block_private: false };
        let strict = EgressPolicy { block_private: true };

        assert_eq!(
            guard_remote_target(&Url::parse("http://127.0.0.1:5000/token").unwrap(), &registry, strict),
            Ok(()),
            "the operator-selected registry origin is always allowed"
        );
        assert_eq!(
            guard_remote_target(
                &Url::parse("http://127.0.0.1:5001/token").unwrap(),
                &registry,
                permissive,
            ),
            Err(RemoteTargetError::Blocked("loopback"))
        );
        assert_eq!(
            guard_remote_target(
                &Url::parse("http://169.254.169.254/token").unwrap(),
                &registry,
                permissive,
            ),
            Err(RemoteTargetError::Blocked("link_local"))
        );
        assert_eq!(
            guard_remote_target(&Url::parse("http://0.0.0.0/token").unwrap(), &registry, permissive),
            Err(RemoteTargetError::Blocked("unspecified"))
        );
        for target in ["http://10.0.0.1/token", "http://[fd00::1]/token"] {
            let target = Url::parse(target).unwrap();
            assert_eq!(guard_remote_target(&target, &registry, permissive), Ok(()));
            assert_eq!(
                guard_remote_target(&target, &registry, strict),
                Err(RemoteTargetError::Blocked("private"))
            );
        }
        for target in [
            "http://8.8.8.8/token",
            "http://[2606:4700:4700::1111]/token",
            "http://cdn.example/token",
        ] {
            assert_eq!(
                guard_remote_target(&Url::parse(target).unwrap(), &registry, strict),
                Ok(()),
                "global literals and hostnames remain allowed: {target}"
            );
        }
        assert_eq!(
            guard_remote_target(
                &Url::parse("http://[::ffff:127.0.0.1]/token").unwrap(),
                &registry,
                permissive,
            ),
            Err(RemoteTargetError::Blocked("loopback"))
        );
    }

    #[test]
    fn token_realm_transport_policy_preserves_http_registries_and_https_realms() {
        let http_realm = Url::parse("http://auth.example/token").unwrap();
        let https_realm = Url::parse("https://auth.example/token").unwrap();

        assert!(token_realm_downgrades_tls(true, &http_realm));
        assert!(!token_realm_downgrades_tls(false, &http_realm));
        assert!(!token_realm_downgrades_tls(true, &https_realm));
    }

    #[tokio::test]
    async fn https_registry_http_token_realm_is_rejected_before_target_request() {
        let target = MockServer::start().await;
        let spec = spec_for(&target, "test/wasm:1.0.0");
        let image = OciImageRef::parse(&spec.image).unwrap();
        let mut session = RegistrySession::new(&image, &spec).unwrap();
        session.registry_origin = Url::parse("https://registry.example").unwrap().origin();
        session.registry_uses_tls = true;
        session.auth = Some(RegistryAuth {
            username: "alice".to_string(),
            password: "s3cret".to_string(),
        });
        let challenge = BearerChallenge {
            realm: format!("{}/token", target.uri()),
            service: Some("registry.example".to_string()),
            scope: None,
        };

        let error = match session.fetch_token(&challenge).await {
            Err(GetError::Definitive(error)) => error,
            Err(GetError::Retryable(error)) => panic!("downgrade must be definitive, got retryable: {error}"),
            Ok(_) => panic!("downgrade must not return a token"),
        };

        assert_eq!(error, "token endpoint blocked by transport policy");
        assert!(
            target.received_requests().await.unwrap().is_empty(),
            "the plaintext token realm must receive zero credential-bearing requests"
        );
    }

    #[tokio::test]
    async fn cross_origin_unsafe_literal_bearer_realm_is_never_requested() {
        let registry = MockServer::start().await;
        let target = MockServer::start().await;
        let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, target.uri());
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .mount(&registry)
            .await;

        let err = fetch_oci_bytes(&spec_for(&registry, "test/wasm:1.0.0"))
            .await
            .unwrap_err();

        assert_eq!(err, "token endpoint blocked by egress policy");
        assert!(
            target.received_requests().await.unwrap().is_empty(),
            "a remote-controlled loopback realm must be rejected before GET"
        );
    }

    #[tokio::test]
    async fn cross_origin_unsafe_literal_blob_redirect_is_never_requested() {
        const REDIRECT_BLOB: &[u8] = b"issue-9-redirect-target";
        let registry = MockServer::start().await;
        let target = MockServer::start().await;
        let manifest = image_manifest(REDIRECT_BLOB, manifest::MEDIA_TYPE_WASM_LAYER);
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/redirect"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest))
            .mount(&registry)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(REDIRECT_BLOB))))
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", format!("{}/blob", target.uri()).as_str()),
            )
            .mount(&registry)
            .await;

        let err = fetch_oci_bytes(&spec_for(&registry, "test/wasm:redirect"))
            .await
            .unwrap_err();

        assert_eq!(err, "OCI redirect blocked by policy");
        assert!(
            target.received_requests().await.unwrap().is_empty(),
            "a remote-controlled loopback redirect must be rejected before GET"
        );
    }

    #[tokio::test]
    async fn cross_origin_hostname_blob_redirect_is_followed() {
        let registry = MockServer::start().await;
        let target = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blob"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
            .mount(&target)
            .await;
        let location = format!("http://cdn.test:{}/blob", target.address().port());
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", location.as_str()))
            .mount(&registry)
            .await;

        let registry_origin = Url::parse(&registry.uri()).unwrap().origin();
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve("cdn.test", *target.address())
            .redirect(blob_redirect_policy(
                registry_origin,
                EgressPolicy { block_private: true },
            ))
            .build()
            .unwrap();
        let response = client.get(format!("{}/start", registry.uri())).send().await.unwrap();

        assert_eq!(response.bytes().await.unwrap(), b"ok".as_slice());
        assert_eq!(target.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn blob_redirect_transport_policy_only_rejects_https_to_http() {
        let source_https = Url::parse("https://registry.example/blob").unwrap();
        let source_http = Url::parse("http://registry.example/blob").unwrap();

        assert!(redirect_downgrades_tls(
            std::slice::from_ref(&source_https),
            &Url::parse("http://registry.example/blob-2").unwrap(),
        ));
        for (source, target) in [
            (&source_http, Url::parse("http://registry.example/blob-2").unwrap()),
            (&source_http, Url::parse("https://registry.example/blob-2").unwrap()),
            (&source_https, Url::parse("https://registry.example/blob-2").unwrap()),
        ] {
            assert!(
                !redirect_downgrades_tls(std::slice::from_ref(source), &target),
                "non-downgrade redirect must remain allowed: {source} -> {target}"
            );
        }
    }

    #[tokio::test]
    async fn authorized_https_blob_redirect_to_http_is_rejected_before_target_request() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let _ = rustls::crypto::ring::default_provider().install_default();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certified.cert.der().clone()], key.into())
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let saw_authorization = Arc::new(AtomicBool::new(false));
        let plaintext_requests = Arc::new(AtomicUsize::new(0));

        let server = {
            let saw_authorization = saw_authorization.clone();
            let plaintext_requests = plaintext_requests.clone();
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = tls.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                let request = String::from_utf8_lossy(&request);
                saw_authorization.store(
                    request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")),
                    Ordering::Release,
                );
                let response = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\n\
                     Location: http://{address}/plaintext-target\r\n\
                     Content-Length: 0\r\n\
                     Connection: close\r\n\r\n"
                );
                tls.write_all(response.as_bytes()).await.unwrap();
                tls.shutdown().await.unwrap();
                drop(tls);

                if let Ok(Ok((_stream, _))) = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
                {
                    plaintext_requests.fetch_add(1, Ordering::AcqRel);
                }
            })
        };

        let source = Url::parse(&format!("https://{address}/blob")).unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .redirect(blob_redirect_policy(
                source.origin(),
                EgressPolicy { block_private: false },
            ))
            .build()
            .unwrap();
        let error = client.get(source).bearer_auth("test-token").send().await.unwrap_err();

        assert!(error.is_redirect(), "downgrade must fail in the redirect policy");
        server.await.unwrap();
        assert!(
            saw_authorization.load(Ordering::Acquire),
            "the source request must carry Authorization so the leak boundary is exercised"
        );
        assert_eq!(
            plaintext_requests.load(Ordering::Acquire),
            0,
            "the plaintext downgrade target must receive zero requests"
        );
    }

    #[tokio::test]
    async fn blob_redirect_policy_allows_exactly_three_hops() {
        let server = MockServer::start().await;
        for (from, to) in [
            ("/three-0", "/three-1"),
            ("/three-1", "/three-2"),
            ("/three-2", "/three-3"),
        ] {
            Mock::given(method("GET"))
                .and(path(from))
                .respond_with(ResponseTemplate::new(307).insert_header("location", to))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/three-3"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
            .expect(1)
            .mount(&server)
            .await;

        let spec = spec_for(&server, "test/wasm:redirect-limit");
        let image = OciImageRef::parse(&spec.image).unwrap();
        let mut session = RegistrySession::new(&image, &spec).unwrap();
        let body = session
            .get(
                true,
                &format!("{}/three-0", server.uri()),
                "application/octet-stream",
                16,
            )
            .await
            .unwrap();

        assert_eq!(body, b"ok");
    }

    #[tokio::test]
    async fn blob_redirect_policy_rejects_fourth_hop_before_target_request() {
        let server = MockServer::start().await;
        for (from, to) in [
            ("/four-0", "/four-1"),
            ("/four-1", "/four-2"),
            ("/four-2", "/four-3"),
            ("/four-3", "/four-never"),
        ] {
            Mock::given(method("GET"))
                .and(path(from))
                .respond_with(ResponseTemplate::new(307).insert_header("location", to))
                .expect(1)
                .mount(&server)
                .await;
        }

        let spec = spec_for(&server, "test/wasm:redirect-limit");
        let image = OciImageRef::parse(&spec.image).unwrap();
        let mut session = RegistrySession::new(&image, &spec).unwrap();
        let err = session
            .get(
                true,
                &format!("{}/four-0", server.uri()),
                "application/octet-stream",
                16,
            )
            .await
            .unwrap_err();

        assert_eq!(err, "OCI redirect blocked by policy");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path() != "/four-never"),
            "the fourth redirect target must not be requested"
        );
    }

    #[tokio::test]
    async fn token_endpoint_5xx_retries_and_then_succeeds() {
        let server = MockServer::start().await;
        let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
        let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
        // Catch-all 401 with the challenge (lower priority).
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .with_priority(5)
            .mount(&server)
            .await;
        // First token exchange returns 503 (transient); it must be retried.
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("alice:s3cret")
        );
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("service", "test-registry"))
            .and(header("authorization", basic.as_str()))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        // Subsequent token exchange succeeds.
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("service", "test-registry"))
            .and(header("authorization", basic.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "test-token"})))
            .with_priority(5)
            .mount(&server)
            .await;
        // Bearer-authed requests succeed (higher priority than the 401).
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.clone()))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
            .mount(&server)
            .await;

        let host = server.uri().trim_start_matches("http://").to_string();
        let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
        let mut spec = spec_for(&server, "test/wasm:1.0.0");
        spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
        let fetched = fetch_oci_bytes(&spec).await.unwrap();
        assert_eq!(fetched.bytes.as_ref(), WASM);
    }

    #[tokio::test]
    async fn token_endpoint_403_is_definitive_no_retry() {
        let server = MockServer::start().await;
        let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .with_priority(5)
            .mount(&server)
            .await;
        // Token endpoint rejects the credentials; a 403 must stop after one attempt.
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("service", "test-registry"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        let host = server.uri().trim_start_matches("http://").to_string();
        let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
        let mut spec = spec_for(&server, "test/wasm:1.0.0");
        spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
        let err = fetch_oci_bytes(&spec).await.unwrap_err();
        assert!(err.contains("token endpoint: status 403"), "got: {err}");
    }

    #[tokio::test]
    async fn retry_backoff_releases_permit_for_another_registry_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;
        use tokio::sync::{Notify, Semaphore};

        let server = MockServer::start().await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let first_a_seen = Arc::new(Notify::new());
        let a_calls = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with({
                let order = order.clone();
                let first_a_seen = first_a_seen.clone();
                let a_calls = a_calls.clone();
                move |_: &wiremock::Request| {
                    let call = a_calls.fetch_add(1, Ordering::SeqCst);
                    order.lock().unwrap().push(if call == 0 { "a1" } else { "a2" });
                    if call == 0 {
                        first_a_seen.notify_one();
                        ResponseTemplate::new(503)
                    } else {
                        ResponseTemplate::new(200).set_body_bytes(b"a".to_vec())
                    }
                }
            })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with({
                let order = order.clone();
                move |_: &wiremock::Request| {
                    order.lock().unwrap().push("b");
                    ResponseTemplate::new(200).set_body_bytes(b"b".to_vec())
                }
            })
            .mount(&server)
            .await;

        let spec = spec_for(&server, "test/wasm:1.0");
        let image = OciImageRef::parse(&spec.image).unwrap();
        let mut session_a = RegistrySession::new(&image, &spec).unwrap();
        let mut session_b = RegistrySession::new(&image, &spec).unwrap();
        let permits = Arc::new(Semaphore::new(1));
        let url_a = format!("{}/a", server.uri());
        let url_b = format!("{}/b", server.uri());

        let permits_a = permits.clone();
        let a = tokio::spawn(async move {
            session_a
                .get_with_permits(&permits_a, false, &url_a, "application/octet-stream", 16)
                .await
        });
        first_a_seen.notified().await;
        let permits_b = permits.clone();
        let b = tokio::spawn(async move {
            session_b
                .get_with_permits(&permits_b, false, &url_b, "application/octet-stream", 16)
                .await
        });

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), b)
                .await
                .expect("B completes while A is backing off")
                .unwrap()
                .unwrap(),
            b"b"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), a)
                .await
                .expect("A completes its retry")
                .unwrap()
                .unwrap(),
            b"a"
        );
        assert_eq!(&*order.lock().unwrap(), &["a1", "b", "a2"]);
    }

    #[tokio::test]
    async fn declared_but_unresolved_pull_secret_fails_closed() {
        let server = MockServer::start().await;
        let mut spec = spec_for(&server, "test/wasm:1.0.0");
        spec = serde_json::from_value(serde_json::json!({
            "image": spec.image,
            "pullSecretRef": {"name": "creds"}
        }))
        .unwrap();
        let err = fetch_oci_bytes(&spec).await.unwrap_err();
        assert!(err.contains("pull secret not resolved"), "got: {err}");
    }

    #[tokio::test]
    async fn blob_digest_mismatch_is_a_hard_failure() {
        let server = MockServer::start().await;
        let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
        // Serve WRONG bytes under the manifest's layer digest.
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"evil".to_vec()))
            .mount(&server)
            .await;
        let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:1.0.0"))
            .await
            .unwrap_err();
        assert!(err.contains("digest mismatch"), "got: {err}");
    }

    #[tokio::test]
    async fn manifest_404_is_definitive_no_retry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/missing"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1) // definitive: exactly one attempt
            .mount(&server)
            .await;
        let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:missing"))
            .await
            .unwrap_err();
        assert!(err.contains("status 404"), "got: {err}");
    }

    #[tokio::test]
    async fn index_resolves_to_first_manifest() {
        let server = MockServer::start().await;
        let img_manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
        let index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": manifest::MEDIA_TYPE_OCI_INDEX,
            "manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": digest_of(&img_manifest)}]
        }))
        .unwrap();
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/multi"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(index.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/manifests/{}", digest_of(&img_manifest))))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(img_manifest.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
            .mount(&server)
            .await;
        let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:multi")).await.unwrap();
        assert_eq!(fetched.bytes.as_ref(), WASM);
        assert_eq!(fetched.manifest_digest, digest_of(&index));

        // The Always re-check must agree with the stored identity, or an
        // index-backed image would reload on every re-check window.
        let recheck = resolve_manifest_digest(&spec_for(&server, "test/wasm:multi"))
            .await
            .unwrap();
        assert_eq!(recheck, fetched.manifest_digest);
    }

    /// An oversized manifest body is rejected by the streaming cap. Because the
    /// helper counts actually-streamed bytes (not `Content-Length`), the same
    /// rejection holds for an absent, false-small, or chunked Content-Length.
    #[tokio::test]
    async fn oversized_manifest_is_rejected_at_the_cap() {
        let server = MockServer::start().await;
        let big = vec![b'x'; MAX_MANIFEST_BYTES + 1];
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
            .mount(&server)
            .await;
        let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:big")).await.unwrap_err();
        assert!(err.contains("body exceeds"), "got: {err}");
    }

    /// An oversized token-endpoint body is rejected by the streaming cap rather
    /// than buffered whole by `resp.json()`.
    #[tokio::test]
    async fn oversized_token_response_is_rejected_at_the_cap() {
        let server = MockServer::start().await;
        let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
        Mock::given(method("GET"))
            .and(path("/v2/test/wasm/manifests/1.0.0"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .mount(&server)
            .await;
        let big = vec![b'x'; MAX_TOKEN_BYTES + 1];
        Mock::given(method("GET"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
            .mount(&server)
            .await;
        let host = server.uri().trim_start_matches("http://").to_string();
        let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
        let mut spec = spec_for(&server, "test/wasm:1.0.0");
        spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
        let err = fetch_oci_bytes(&spec).await.unwrap_err();
        assert!(err.contains("response exceeds"), "got: {err}");
    }
}
