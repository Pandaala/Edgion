//! Background GeoIP database updater.
//!
//! One detached task. Never spawned when `update_url` is empty. A cycle sends a
//! conditional GET only while its local validator context is eligible; a valid
//! 304 sleeps normally, while an untrusted 304 is a fetch failure with cold
//! backoff and an unconditional next request. A 200 streams to `<db_path>.tmp`
//! under hard bounds, is validated structurally, renamed atomically, and swapped
//! into the provider. A failed validation never renames: half a broken database
//! is worse than none.
//!
//! Free-space precheck: implemented via `rustix::fs::statvfs`, which is already
//! a workspace dependency (used by the `LocalFile` log sink's disk-usage probe).
//! During a swap the volume can transiently hold the bounded `.tmp` plus two
//! copies of the current database around the rename. The precheck therefore
//! requires `max_db_bytes + 2 * current_database_size`, or `max_db_bytes` when
//! no live database exists yet.
//!
//! Jitter: applied to the initial fetch as well as every subsequent interval.
//! Without it, a fleet-wide rolling restart would issue N simultaneous
//! unconditional GETs at process start.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use reqwest::header::{
    HeaderValue, AUTHORIZATION, CONTENT_ENCODING, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};

/// `<db_path>.tmp` — deliberately a sibling, because `rename(2)` is only atomic
/// within one filesystem. A cross-device rename degrades to a copy and loses the
/// guarantee the hot swap depends on.
pub(crate) fn tmp_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// `<db_path>.meta` — the persisted ETag / Last-Modified validator. Without it,
/// a rolling restart on an `emptyDir` volume turns every conditional GET into an
/// unconditional one: 200 pods x 60 MB in a few minutes.
pub(crate) fn meta_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

/// Backoff before the first successful load: 1s doubling, capped at 30s.
/// `retry_interval_sec` (600s) is tuned for steady state and would leave a
/// cold-started pod unprotected for ten minutes.
pub(crate) fn cold_start_backoff(attempt: u32) -> Duration {
    let secs = (1u64 << attempt.min(5)).min(30);
    Duration::from_secs(secs)
}

/// A persisted validator is meaningful only while both halves of the local
/// fallback are usable: the durable database still exists as a regular file,
/// and the process has an active mmap-backed provider.
fn validator_context_ready(db_path: &Path, provider_active: bool) -> bool {
    provider_active && std::fs::metadata(db_path).map(|meta| meta.is_file()).unwrap_or(false)
}

/// Parse one Secret-derived Authorization value and attach its provenance
/// before it crosses a storage, clone, or transport boundary.
fn sensitive_authorization_header(env_name: &str, raw: &str) -> Result<HeaderValue, String> {
    let mut value = HeaderValue::from_str(raw)
        .map_err(|_| format!("env var {env_name} does not contain a valid Authorization header value"))?;
    value.set_sensitive(true);
    Ok(value)
}

/// Structural validation only.
///
/// Structural only, by design: there is deliberately NO content probe. No
/// address resolves in an arbitrary operator-supplied database — a trimmed
/// regional mirror, an IPv6-only build, a DB-IP/IP2Location build, or an
/// internal database mapping only RFC1918 space are all legitimate, and any
/// probe address would reject them forever. The checks below catch the failure
/// this step exists for: a truncated, empty, or non-MMDB candidate.
pub(crate) fn validate_structural(path: &Path) -> Result<(), String> {
    // SAFETY: see the note in `provider.rs` — the candidate file is written by
    // this process and is not mutated in place.
    let reader =
        unsafe { maxminddb::Reader::open_mmap(path) }.map_err(|e| format!("candidate mmdb unreadable: {e}"))?;
    let meta = reader.metadata();

    if meta.database_type.is_empty() {
        return Err("candidate mmdb has an empty database_type".to_string());
    }
    if meta.node_count == 0 {
        return Err("candidate mmdb has node_count 0".to_string());
    }
    if meta.binary_format_major_version == 0 {
        return Err("candidate mmdb has no binary format version".to_string());
    }
    if meta.build_epoch == 0 {
        return Err("candidate mmdb has build_epoch 0".to_string());
    }
    Ok(())
}

/// Sleep for `base` plus jitter, so N processes started at the same instant do
/// not fire simultaneous requests.
///
/// This intentionally jitters the zero-duration case too (the initial fetch):
/// a fleet-wide rolling restart calls this with `base == 0` for the first
/// cycle, and without an unconditional jitter floor every pod would issue its
/// first GET in lockstep. Steady-state intervals get up to 20% jitter on top
/// of `base`.
async fn sleep_with_jitter(base: Duration) {
    use rand::Rng;
    let jitter = if base.is_zero() {
        Duration::from_secs_f64(rand::rng().random_range(0.0..5.0))
    } else {
        Duration::from_secs_f64(base.as_secs_f64() * rand::rng().random_range(0.0..0.2))
    };
    tokio::time::sleep(base + jitter).await;
}

/// Refuse to start a download when the volume cannot hold a full swap.
///
/// During a swap the volume transiently holds the old (still-existing until
/// `rename(2)`) database, the new `.tmp` being written, and — briefly, after
/// rename — both the new file and any reader still mapping the old inode. That
/// headroom is `1x max_db_bytes` (the `.tmp` being streamed, capped at
/// `max_db_bytes`) `+ 2x` the *current on-disk database size* (the old file,
/// counted twice to cover the transient double-hold around the rename) — not
/// `3x max_db_bytes`, which overshoots whenever the live database is smaller
/// than the cap (the common case: `docs/en/ops-guide/gateway/geoip.md` sizes
/// storage off the database, not the cap). When no database exists yet (first
/// start), there is nothing to double-hold, so the requirement falls back to
/// `1x max_db_bytes` for the `.tmp` alone. `db_path` should be the target
/// database path; its parent directory is used for `statvfs` and must exist.
/// Pure arithmetic, split out from `ensure_free_space` so the formula itself
/// is unit-testable without touching a real filesystem or `statvfs`.
fn free_space_needed(max_db_bytes: u64, current_db_size: u64) -> u64 {
    max_db_bytes.saturating_add(current_db_size.saturating_mul(2))
}

#[cfg(unix)]
fn ensure_free_space(db_path: &Path, max_db_bytes: u64) -> Result<(), String> {
    let dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stat = rustix::fs::statvfs(dir).map_err(|e| format!("statvfs {}: {e}", dir.display()))?;
    let avail_bytes = stat.f_bavail.saturating_mul(stat.f_frsize);
    let current_db_size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let needed = free_space_needed(max_db_bytes, current_db_size);
    if avail_bytes < needed {
        return Err(format!(
            "insufficient free space on {}: {avail_bytes} bytes available, need ~{needed} \
             (max_db_bytes + 2x current database size)",
            dir.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_free_space(_db_path: &Path, _max_db_bytes: u64) -> Result<(), String> {
    Ok(())
}

/// `EgressResolver` is a DNS resolver, so it only sees hostname targets. An
/// IP-literal `update_url` (e.g. `http://169.254.169.254/...`, cloud metadata)
/// never triggers a DNS lookup and would bypass egress control entirely, so
/// literals are classified explicitly here — mirroring
/// `guard_remote_target` in `plugins/http/wasm/oci/mod.rs`.
fn classify_literal_destination(url: &str) -> Result<(), String> {
    use crate::plugins::http::common::outbound::egress::default_block_private;
    use crate::util::net::{classify_ip, IpClass};
    use url::Host;

    let parsed = url::Url::parse(url).map_err(|e| format!("invalid update_url: {e}"))?;
    let ip = match parsed.host() {
        Some(Host::Domain(_)) => return Ok(()),
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
        None => return Err("update_url has no host".to_string()),
    };

    let reason = match classify_ip(ip) {
        // Integration tests (and this module's own unit tests) point `update_url`
        // at `127.0.0.1`; the same escape hatch `EgressResolver` honors must apply
        // here, or the guard kills our own test suite.
        IpClass::Loopback if cfg!(test) || edgion_common::test_mode::is_integration_testing_mode() => None,
        IpClass::Loopback => Some("loopback"),
        IpClass::LinkLocal => Some("link_local"),
        IpClass::Unspecified => Some("unspecified"),
        IpClass::Private | IpClass::UniqueLocal if default_block_private() => Some("private"),
        IpClass::Private | IpClass::UniqueLocal | IpClass::Global => None,
    };

    match reason {
        Some(reason) => {
            crate::observe::metrics::record_egress_blocked(reason);
            Err(format!("update_url resolves to a blocked destination ({reason})"))
        }
        None => Ok(()),
    }
}

use edgion_common::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use super::config::GeoIpConfig;
use super::provider::GeoIpProvider;

#[derive(Debug)]
pub(crate) enum CycleOutcome {
    /// 304 — nothing to do, disk untouched.
    NotModified,
    /// A new database was validated, renamed, and swapped in.
    Swapped,
}

/// Failure classes. These map onto the bounded `reason` labels of
/// `edgion_geoip_update_failed_total`; without the distinction an operator
/// cannot tell a slow link from a rejected file. Internal recovery variants
/// may deliberately share an existing public reason.
#[derive(Debug)]
pub(crate) enum UpdateError {
    Fetch(String),
    Encoding(String),
    SizeCap(String),
    Stall(String),
    Validation(String),
    Rename(String),
    /// The free-space precheck refused to start a download. Distinct from
    /// `Fetch`: no request was ever issued, and the fix is "grow the volume",
    /// not "check the network".
    Disk(String),
    /// A 304 cannot be used without both a request validator and a usable
    /// local database/provider pair. It retains the existing `fetch` metric
    /// reason while forcing cold recovery semantics.
    InvalidNotModified(String),
}

impl UpdateError {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::Fetch(_) => "fetch",
            Self::Encoding(_) => "encoding",
            Self::SizeCap(_) => "size_cap",
            Self::Stall(_) => "stall",
            Self::Validation(_) => "validation",
            Self::Rename(_) => "rename",
            Self::Disk(_) => "disk",
            Self::InvalidNotModified(_) => "fetch",
        }
    }

    /// The detail string carried by every variant. Named accessor rather than a
    /// `Display`/derived-`Debug` read so the compiler counts the payload as used
    /// (a plain `#[derive(Debug)]` read does not count for dead-code analysis).
    pub(crate) fn detail(&self) -> &str {
        match self {
            Self::Fetch(s)
            | Self::Encoding(s)
            | Self::SizeCap(s)
            | Self::Stall(s)
            | Self::Validation(s)
            | Self::Rename(s)
            | Self::Disk(s)
            | Self::InvalidNotModified(s) => s,
        }
    }

    fn requires_cold_backoff(&self) -> bool {
        matches!(self, Self::InvalidNotModified(_))
    }
}

/// Opaque equality metadata for the exact configured representation inputs.
/// Computed once at updater construction; never log this digest or its inputs.
/// SHA-256 does not encrypt credentials or protect against offline guessing.
fn source_identity(url: &str, auth_header: Option<&HeaderValue>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"edgion-geoip-update-source-v1\0");
    digest.update((url.len() as u64).to_be_bytes());
    digest.update(url.as_bytes());
    match auth_header {
        Some(value) => {
            digest.update([1]);
            digest.update((value.as_bytes().len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        None => digest.update([0]),
    }
    hex::encode(digest.finalize())
}

/// The conditional-GET validator (ETag / Last-Modified), persisted next to the
/// database as `<db_path>.meta`.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateState {
    #[serde(default)]
    pub(crate) source_identity: Option<String>,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
}

impl UpdateState {
    /// Recover validators only for the current source and usable local state.
    /// Only an absent sidecar permits the deliberate same-source mtime fallback;
    /// existing untrusted metadata must not regain eligibility through mtime.
    pub(crate) fn load(meta_path: &Path, db_path: &Path, provider_active: bool, current_source: &str) -> Self {
        if !validator_context_ready(db_path, provider_active) {
            let state = Self::default();
            state.persist(meta_path);
            return state;
        }
        match std::fs::read(meta_path) {
            Ok(bytes) => {
                let mut state = serde_json::from_slice::<Self>(&bytes).unwrap_or_default();
                if !state.matches_source(current_source) {
                    state.clear_and_persist(meta_path);
                }
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let last_modified = std::fs::metadata(db_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .map(httpdate::fmt_http_date);
                Self {
                    source_identity: Some(current_source.to_string()),
                    etag: None,
                    last_modified,
                }
            }
            Err(_) => {
                let state = Self::default();
                state.persist(meta_path);
                state
            }
        }
    }

    fn matches_source(&self, current_source: &str) -> bool {
        self.source_identity.as_deref() == Some(current_source)
    }

    /// Called only after validation, rename and successful provider publication.
    fn record_swap(
        &mut self,
        meta_path: &Path,
        current_source: &str,
        etag: Option<String>,
        last_modified: Option<String>,
    ) {
        self.source_identity = Some(current_source.to_string());
        self.etag = etag;
        self.last_modified = last_modified;
        self.persist(meta_path);
    }

    /// Persist source-bound validators best-effort. This is not a transaction
    /// with the database rename: a failed write may lose restart optimization.
    /// Untrusted existing metadata stays unconditional; absent metadata retains
    /// the explicitly documented same-source mtime assumption.
    pub(crate) fn persist(&self, meta_path: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = std::fs::write(meta_path, bytes);
        }
    }

    fn clear_and_persist(&mut self, meta_path: &Path) {
        self.etag = None;
        self.last_modified = None;
        self.persist(meta_path);
    }
}

fn prepare_validator_state(state: &mut UpdateState, meta_path: &Path, validators_eligible: bool) {
    if !validators_eligible {
        state.clear_and_persist(meta_path);
    }
}

fn header_string(resp: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

pub struct GeoIpUpdater {
    cfg: GeoIpConfig,
    db_path: PathBuf,
    url: String,
    source_identity: String,
    auth_header: Option<HeaderValue>,
    client: reqwest::Client,
    provider: &'static GeoIpProvider,
}

impl GeoIpUpdater {
    pub fn new(cfg: GeoIpConfig, provider: &'static GeoIpProvider) -> Result<Self, String> {
        cfg.validate_updater_settings()?;
        let db_path = cfg
            .db_path
            .clone()
            .ok_or("geoip.db_path is required when the updater runs")?;
        let url = cfg.update_url.clone().ok_or("geoip.update_url is empty")?.into_inner();

        // `EgressResolver` is a DNS resolver, so it only sees hostname targets.
        // An IP-literal URL never triggers a lookup and would bypass egress
        // control entirely, so literals are classified explicitly here.
        classify_literal_destination(&url)?;

        let auth_header = match cfg.update_auth_header_env.as_deref() {
            Some(var) => {
                let raw = std::env::var(var).map_err(|_| format!("env var {var} is not set"))?;
                Some(sensitive_authorization_header(var, &raw)?)
            }
            None => None,
        };

        let source_identity = source_identity(&url, auth_header.as_ref());

        // `classify_literal_destination` above only sees IP-literal hosts;
        // reqwest never runs a literal through DNS. A hostname `update_url`
        // instead needs its egress policy applied at resolution time, which is
        // what installing `EgressResolver` as the client's DNS resolver gives
        // us — the two checks are complementary, not redundant (design spec §4.3).
        let dns_resolver: std::sync::Arc<dyn reqwest::dns::Resolve> =
            std::sync::Arc::new(crate::plugins::http::common::outbound::egress::EgressResolver::new(
                edgion_common::dns::ResolverSpec::System,
                crate::plugins::http::common::outbound::egress::EgressPolicy {
                    block_private: crate::plugins::http::common::outbound::egress::default_block_private(),
                },
                crate::plugins::http::common::outbound::egress::egress_dns_cache_ttl(),
            ));

        let client = build_outbound_client(OutboundClientConfig {
            connect_timeout: Some(Duration::from_secs(cfg.connect_timeout_sec)),
            // total_timeout stays None on purpose: it covers the whole request
            // including the body, so a 60-80 MB database on a slow link would
            // abort every cycle forever. The deadline is enforced below instead.
            total_timeout: None,
            // Defaults to false; egress policy requires it explicitly on.
            disable_proxy: true,
            // OutboundRedirect::Deny is already the default.
            dns_resolver: Some(dns_resolver),
            ..Default::default()
        })
        .map_err(|e| format!("build geoip outbound client: {e}"))?;

        Ok(Self {
            cfg,
            db_path,
            url,
            source_identity,
            auth_header,
            client,
            provider,
        })
    }

    pub fn spawn(self) {
        tokio::spawn(async move { self.run().await });
    }

    async fn run(self) {
        let meta = meta_path(&self.db_path);
        let mut state = UpdateState::load(&meta, &self.db_path, self.provider.is_loaded(), &self.source_identity);
        let mut cold_attempt = 0u32;

        loop {
            // Jitter the initial fetch too, not only retries: without it a
            // 200-pod rolling restart issues 200 simultaneous unconditional GETs.
            sleep_with_jitter(Duration::from_secs(0)).await;

            match tokio::time::timeout(
                Duration::from_secs(self.cfg.download_deadline_sec),
                self.run_cycle(&mut state),
            )
            .await
            {
                Ok(Ok(CycleOutcome::Swapped)) | Ok(Ok(CycleOutcome::NotModified)) => {
                    cold_attempt = 0;
                    sleep_with_jitter(Duration::from_secs(self.cfg.update_interval_sec)).await;
                }
                Ok(Err(e)) => {
                    // tracing-allow: background task, not a request path
                    tracing::error!(reason = e.reason(), detail = e.detail(), "geoip update failed");
                    crate::observe::metrics::record_geoip_update_failed(e.reason());
                    self.sleep_after_failure(&mut cold_attempt, e.requires_cold_backoff())
                        .await;
                }
                Err(_) => {
                    // Distinct from `Stall` (`UpdateError::Stall`, no *chunk*
                    // progress within `stall_timeout_sec`): this is the whole
                    // cycle exceeding `download_deadline_sec` even while making
                    // steady (slow) progress — the opposite fix applies, so it
                    // gets its own reason label.
                    // tracing-allow: background task, not a request path
                    tracing::error!("geoip update exceeded download_deadline_sec");
                    crate::observe::metrics::record_geoip_update_failed("deadline");
                    self.sleep_after_failure(&mut cold_attempt, false).await;
                }
            }
        }
    }

    async fn sleep_after_failure(&self, cold_attempt: &mut u32, force_cold: bool) {
        let base = failure_sleep_base(
            self.provider.is_loaded(),
            force_cold,
            Duration::from_secs(self.cfg.retry_interval_sec),
            cold_attempt,
        );
        sleep_with_jitter(base).await;
    }
}

fn failure_sleep_base(
    provider_active: bool,
    force_cold: bool,
    retry_interval: Duration,
    cold_attempt: &mut u32,
) -> Duration {
    if provider_active && !force_cold {
        retry_interval
    } else {
        let delay = cold_start_backoff(*cold_attempt);
        *cold_attempt = cold_attempt.saturating_add(1);
        delay
    }
}

fn build_request(
    client: &reqwest::Client,
    url: &str,
    auth_header: Option<&HeaderValue>,
    state: &UpdateState,
    validators_eligible: bool,
) -> (reqwest::RequestBuilder, bool) {
    let mut req = client.get(url);
    if let Some(value) = auth_header {
        req = req.header(AUTHORIZATION, value.clone());
    }

    if !validators_eligible {
        return (req, false);
    }
    if let Some(etag) = &state.etag {
        (req.header(IF_NONE_MATCH, etag), true)
    } else if let Some(last_modified) = &state.last_modified {
        (req.header(IF_MODIFIED_SINCE, last_modified), true)
    } else {
        (req, false)
    }
}

fn handle_not_modified_response(
    state: &mut UpdateState,
    meta_path: &Path,
    sent_validator: bool,
    validator_context_ready: bool,
) -> Result<CycleOutcome, UpdateError> {
    if sent_validator && validator_context_ready {
        return Ok(CycleOutcome::NotModified);
    }

    state.clear_and_persist(meta_path);
    Err(UpdateError::InvalidNotModified(
        "received 304 without eligible conditional request state".to_string(),
    ))
}

impl GeoIpUpdater {
    pub(crate) async fn run_cycle(&self, state: &mut UpdateState) -> Result<CycleOutcome, UpdateError> {
        let tmp = tmp_path(&self.db_path);
        let meta = meta_path(&self.db_path);

        // A partial .tmp from a crashed pod consumes the space every subsequent
        // cycle needs, wedging updates permanently. Clear it first, always.
        let _ = std::fs::remove_file(&tmp);

        // Re-check on every cycle. Suppressing conditional headers is not
        // enough: stale in-memory state could otherwise become eligible again
        // after a failed fetch if the database/provider recovered.
        let mut validators_eligible = validator_context_ready(&self.db_path, self.provider.is_loaded())
            && state.matches_source(&self.source_identity);
        prepare_validator_state(state, &meta, validators_eligible);

        // Refuse to start a download the volume cannot hold a full swap of. Not
        // fatal to the process — the updater task keeps running and retries on
        // the normal failure backoff — but it does abort this cycle: a
        // `statvfs` error (e.g. the parent dir does not exist yet on a fresh
        // volume) is treated the same as insufficient space below.
        if let Err(e) = ensure_free_space(&self.db_path, self.cfg.max_db_bytes) {
            return Err(UpdateError::Disk(e));
        }

        // The free-space check above is synchronous but the local state may
        // still have changed since the first eligibility read. Revalidate at
        // the request-construction boundary and clear again if needed.
        validators_eligible = validator_context_ready(&self.db_path, self.provider.is_loaded())
            && state.matches_source(&self.source_identity);
        prepare_validator_state(state, &meta, validators_eligible);
        let (req, sent_validator) = build_request(
            &self.client,
            &self.url,
            self.auth_header.as_ref(),
            state,
            validators_eligible,
        );

        // `reqwest::Error`'s `Display` appends " for url (...)", and `update_url`
        // may carry an operator-embedded credential (design spec §2) or a
        // signed-query mirror URL. `without_url()` strips that suffix before the
        // error is ever stringified, so it never reaches the ERROR log this
        // error eventually flows through (`run`, below).
        let resp = req
            .send()
            .await
            .map_err(|e| UpdateError::Fetch(e.without_url().to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            let validator_context_ready = validator_context_ready(&self.db_path, self.provider.is_loaded());
            return handle_not_modified_response(state, &meta, sent_validator, validator_context_ready);
        }
        if !resp.status().is_success() {
            return Err(UpdateError::Fetch(format!("status {}", resp.status())));
        }

        // Most mirrors serve .mmdb.gz or negotiate gzip. Streaming a compressed
        // body straight to .tmp yields a file that fails validation forever with
        // an opaque error, so reject it loudly instead of decompressing.
        if let Some(enc) = resp.headers().get(CONTENT_ENCODING) {
            let enc = enc.to_str().unwrap_or("");
            if !enc.is_empty() && !enc.eq_ignore_ascii_case("identity") {
                return Err(UpdateError::Encoding(format!("unsupported Content-Encoding: {enc}")));
            }
        }

        let new_etag = header_string(&resp, ETAG);
        let new_last_modified = header_string(&resp, LAST_MODIFIED);

        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| UpdateError::Fetch(format!("create tmp: {e}")))?;
        let mut written: u64 = 0;
        let mut stream = resp.bytes_stream();

        loop {
            let next = tokio::time::timeout(Duration::from_secs(self.cfg.stall_timeout_sec), stream.next()).await;
            let chunk = match next {
                Err(_) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(UpdateError::Stall("no progress within stall_timeout_sec".into()));
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(UpdateError::Fetch(e.without_url().to_string()));
                }
                Ok(Some(Ok(c))) => c,
            };

            written += chunk.len() as u64;
            // Enforced WHILE streaming, never after: the point is to not hold or
            // write an unbounded body. Exceeding the cap fails the cycle; it
            // never truncates and never falls back to an uncapped read.
            if written > self.cfg.max_db_bytes {
                let _ = std::fs::remove_file(&tmp);
                return Err(UpdateError::SizeCap(format!(
                    "exceeded max_db_bytes ({})",
                    self.cfg.max_db_bytes
                )));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| UpdateError::Fetch(format!("write tmp: {e}")))?;
        }
        file.flush().await.map_err(|e| UpdateError::Fetch(e.to_string()))?;
        file.sync_all().await.map_err(|e| UpdateError::Fetch(e.to_string()))?;
        drop(file);

        validate_structural(&tmp).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            UpdateError::Validation(e)
        })?;

        std::fs::rename(&tmp, &self.db_path).map_err(|e| UpdateError::Rename(e.to_string()))?;

        self.provider.load_from(&self.db_path).map_err(UpdateError::Rename)?;

        state.record_swap(&meta, &self.source_identity, new_etag, new_last_modified);

        crate::observe::metrics::set_geoip_db_loaded(true);
        if let Some(epoch) = self.provider.build_epoch() {
            crate::observe::metrics::set_geoip_db_build_epoch(epoch);
        }
        Ok(CycleOutcome::Swapped)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/geoip/updater_test.rs"]
mod tests;
