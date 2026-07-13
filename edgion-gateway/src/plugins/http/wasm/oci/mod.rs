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
//! requests follow up to 3 (registries routinely 307 blobs to CDN/S3). That
//! stays SSRF-safe because the `EgressResolver` classifies every hop's
//! addresses at DNS resolution time, and reqwest strips `Authorization` on
//! cross-host redirects.

pub mod auth;
pub mod cache;
pub mod manifest;

use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use edgion_resources::plugin_configs::{OciImageRef, WasmOciSpec};

use self::auth::{parse_bearer_challenge, select_registry_auth, BearerChallenge, RegistryAuth};
use self::cache::CacheKind;
use self::manifest::{
    extract_wasm_from_tar_gz, parse_manifest, select_wasm_layer, OciManifest, ParsedManifest, WasmLayer,
    MANIFEST_ACCEPT,
};

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

/// Result of a successful OCI fetch.
#[derive(Debug)]
pub struct OciFetched {
    /// Final wasm bytecode (post-extraction for the compat variant).
    pub bytes: Vec<u8>,
    /// The digest (`sha256:<hex>`) that the image reference resolves to at
    /// the TOP level — an image manifest digest, or an index digest when the
    /// reference points at a multi-platform index. This is the value the
    /// `Always` pullPolicy re-check (`resolve_manifest_digest`, which never
    /// descends into an index) compares against, so it must stay the
    /// top-level identity rather than the child image-manifest digest
    /// selected out of an index.
    pub manifest_digest: String,
}

/// Fetch the wasm bytes for `spec`, disk-cache-first. The caller
/// (`vm_manager`) already serializes same-key loads via the `LoadSlot`.
pub async fn fetch_oci_bytes(spec: &WasmOciSpec) -> Result<OciFetched, String> {
    let image = OciImageRef::parse(&spec.image).map_err(|e| format!("source.oci.image: {e}"))?;
    let mut session = RegistrySession::new(&image, spec)?;

    // Manifest: disk-first for digest refs (zero network on a hit); tag refs
    // resolve over the network.
    let (manifest_bytes, manifest_digest) = fetch_manifest(&mut session, &image).await?;

    // An index costs one more manifest hop; wasm images carry no platform
    // semantics, so take the first entry (Istio behavior). `manifest_digest`
    // stays the TOP-LEVEL digest (image manifest or index) — it is the
    // identity the `Always` re-check (`resolve_manifest_digest`, which never
    // descends an index) compares against, so it must not be overwritten
    // with the child image-manifest digest here.
    let image_manifest: OciManifest = match parse_manifest(&manifest_bytes)? {
        ParsedManifest::Image(m) => m,
        ParsedManifest::Index(entries) => {
            let digest = entries[0].digest.clone();
            let bytes = fetch_manifest_by_digest(&mut session, &digest).await?;
            match parse_manifest(&bytes)? {
                ParsedManifest::Image(m) => m,
                ParsedManifest::Index(_) => return Err("nested image index is not supported".to_string()),
            }
        }
    };

    let (descriptor, compat) = match select_wasm_layer(&image_manifest)? {
        WasmLayer::Raw(d) => (d.clone(), false),
        WasmLayer::TarGz(d) => (d.clone(), true),
    };
    let blob = fetch_blob(&mut session, &descriptor.digest).await?;
    let bytes = if compat { extract_wasm_from_tar_gz(&blob)? } else { blob };
    Ok(OciFetched { bytes, manifest_digest })
}

/// Resolve the current manifest digest for `spec` (the `Always` pullPolicy
/// re-check). One small manifest request; no blob traffic.
pub async fn resolve_manifest_digest(spec: &WasmOciSpec) -> Result<String, String> {
    let image = OciImageRef::parse(&spec.image).map_err(|e| format!("source.oci.image: {e}"))?;
    if let Some(d) = &image.digest {
        return Ok(d.clone());
    }
    let mut session = RegistrySession::new(&image, spec)?;
    let (_, digest) = fetch_manifest(&mut session, &image).await?;
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
    /// `https://host[:port]` (`http://` for loopback registries).
    base: String,
    repo: String,
    auth: Option<RegistryAuth>,
    bearer: Option<String>,
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
        Ok(RegistrySession {
            base: registry_base(&api_host),
            repo: image.repository.clone(),
            auth,
            bearer: None,
            client_deny: outbound_client(OutboundRedirect::Deny, pull_timeout())?,
            client_blob: outbound_client(OutboundRedirect::Limit(3), pull_timeout())?,
        })
    }

    /// GET with credentials, the 401 Bearer-challenge flow, and the fixed
    /// retry policy. Shares the global wasm fetch semaphore per request.
    async fn get(&mut self, blob: bool, url: &str, accept: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        let _permit = super::vm_manager::url_fetch_permits()
            .acquire()
            .await
            .map_err(|_| "wasm fetch semaphore closed".to_string())?;
        let mut last_error = String::new();
        let attempts = pull_attempts();
        for attempt in 0..attempts {
            if attempt > 0 {
                let delay = BACKOFF_MS.saturating_mul(1u64 << (attempt - 1).min(10));
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            match self.get_authed(blob, url, accept, max_bytes).await {
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
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| GetError::Retryable(format!("read body of {url}: {e}")))?;
        if bytes.len() > max_bytes {
            return Err(GetError::Definitive(format!(
                "GET {url}: body exceeds {max_bytes} bytes"
            )));
        }
        Ok(bytes.to_vec())
    }

    async fn send(&self, blob: bool, url: &str, accept: &str) -> Result<reqwest::Response, GetError> {
        let client = if blob { &self.client_blob } else { &self.client_deny };
        let mut req = client.get(url).header(reqwest::header::ACCEPT, accept);
        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        } else if let Some(a) = &self.auth {
            req = req.basic_auth(&a.username, Some(&a.password));
        }
        req.send()
            .await
            .map_err(|e| GetError::Retryable(format!("GET {url}: {e}")))
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
        let mut req = self.client_deny.get(&challenge.realm).query(&query);
        if let Some(a) = &self.auth {
            req = req.basic_auth(&a.username, Some(&a.password));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| GetError::Retryable(format!("token endpoint: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            // A failed token exchange is an auth failure — definitive.
            return Err(GetError::Definitive(format!("token endpoint: status {status}")));
        }
        let parsed: TokenResponse = resp
            .json()
            .await
            .map_err(|e| GetError::Retryable(format!("token response: {e}")))?;
        parsed
            .token
            .or(parsed.access_token)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| GetError::Definitive("token response contains no token".to_string()))
    }
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

fn outbound_client(
    redirect: crate::link_sys::http_client::OutboundRedirect,
    total_timeout: Duration,
) -> Result<reqwest::Client, String> {
    use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig};
    use crate::plugins::http::common::outbound::egress::{
        default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
    };
    use edgion_common::dns::ResolverSpec;

    let egress: Arc<dyn reqwest::dns::Resolve> = Arc::new(EgressResolver::new(
        ResolverSpec::System,
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    ));
    build_outbound_client(OutboundClientConfig {
        pool_max_idle_per_host: Some(4),
        connect_timeout: Some(Duration::from_secs(5)),
        total_timeout: Some(total_timeout),
        redirect,
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

async fn fetch_manifest(session: &mut RegistrySession, image: &OciImageRef) -> Result<(Vec<u8>, String), String> {
    if let Some(digest) = &image.digest {
        let bytes = fetch_manifest_by_digest(session, digest).await?;
        return Ok((bytes, digest.clone()));
    }
    let tag = image.tag.as_deref().unwrap_or("latest");
    let url = format!("{}/v2/{}/manifests/{}", session.base, session.repo, tag);
    let bytes = session.get(false, &url, MANIFEST_ACCEPT, MAX_MANIFEST_BYTES).await?;
    let hex_part = hex::encode(Sha256::digest(&bytes));
    cache::store(CacheKind::Manifest, &hex_part, &bytes);
    Ok((bytes, format!("sha256:{hex_part}")))
}

async fn fetch_manifest_by_digest(session: &mut RegistrySession, digest: &str) -> Result<Vec<u8>, String> {
    let hex_part = digest.strip_prefix("sha256:").unwrap_or(digest);
    if let Some(bytes) = cache::load(CacheKind::Manifest, hex_part) {
        return Ok(bytes);
    }
    let url = format!("{}/v2/{}/manifests/{}", session.base, session.repo, digest);
    let bytes = session.get(false, &url, MANIFEST_ACCEPT, MAX_MANIFEST_BYTES).await?;
    verify_digest(&bytes, digest)?;
    cache::store(CacheKind::Manifest, hex_part, &bytes);
    Ok(bytes)
}

async fn fetch_blob(session: &mut RegistrySession, digest: &str) -> Result<Vec<u8>, String> {
    let hex_part = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unsupported layer digest algorithm '{digest}' (only sha256)"))?;
    if let Some(bytes) = cache::load(CacheKind::Blob, hex_part) {
        return Ok(bytes);
    }
    let url = format!("{}/v2/{}/blobs/{}", session.base, session.repo, digest);
    let bytes = session
        .get(true, &url, "application/octet-stream", MAX_BLOB_BYTES)
        .await?;
    verify_digest(&bytes, digest)?;
    cache::store(CacheKind::Blob, hex_part, &bytes);
    Ok(bytes)
}

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
        assert_eq!(fetched.bytes, WASM);
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
        assert_eq!(fetched.bytes, WASM);
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
        assert_eq!(fetched.bytes, WASM);
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
        assert_eq!(fetched.bytes, WASM);
        assert_eq!(fetched.manifest_digest, digest_of(&index));

        // The Always re-check must agree with the stored identity, or an
        // index-backed image would reload on every re-check window.
        let recheck = resolve_manifest_digest(&spec_for(&server, "test/wasm:multi"))
            .await
            .unwrap();
        assert_eq!(recheck, fetched.manifest_digest);
    }
}
