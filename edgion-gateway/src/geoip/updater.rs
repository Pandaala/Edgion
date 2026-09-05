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

/// The conditional-GET validator (ETag / Last-Modified), persisted next to the
/// database as `<db_path>.meta`.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateState {
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
}

impl UpdateState {
    /// Load the persisted validator only when a regular durable database and
    /// an active provider both exist. When
    /// `meta_path` is absent in that usable state, seed `last_modified` from
    /// the database file's mtime so the first cycle can still be conditional.
    /// A valid empty sidecar deliberately suppresses this mtime fallback.
    pub(crate) fn load(meta_path: &Path, db_path: &Path, provider_active: bool) -> Self {
        if !validator_context_ready(db_path, provider_active) {
            let state = Self::default();
            state.persist(meta_path);
            return state;
        }
        if let Ok(bytes) = std::fs::read(meta_path) {
            if let Ok(state) = serde_json::from_slice::<UpdateState>(&bytes) {
                return state;
            }
        }
        let last_modified = std::fs::metadata(db_path)
            .and_then(|m| m.modified())
            .ok()
            .map(httpdate::fmt_http_date);
        Self {
            etag: None,
            last_modified,
        }
    }

    /// Persist the validator. Best-effort: a failed write only costs the next
    /// cycle a conditional GET turning unconditional, not correctness.
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
        let mut state = UpdateState::load(&meta, &self.db_path, self.provider.is_loaded());
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
        let mut validators_eligible = validator_context_ready(&self.db_path, self.provider.is_loaded());
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
        validators_eligible = validator_context_ready(&self.db_path, self.provider.is_loaded());
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

        state.etag = new_etag;
        state.last_modified = new_last_modified;
        state.persist(&meta);

        crate::observe::metrics::set_geoip_db_loaded(true);
        if let Some(epoch) = self.provider.build_epoch() {
            crate::observe::metrics::set_geoip_db_build_epoch(epoch);
        }
        Ok(CycleOutcome::Swapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_city_db() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb")
    }

    #[test]
    fn structural_validation_accepts_a_real_database() {
        assert!(validate_structural(&fixture_city_db()).is_ok());
    }

    #[test]
    fn structural_validation_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.mmdb");
        std::fs::write(&path, b"not an mmdb at all").unwrap();
        assert!(validate_structural(&path).is_err());
    }

    #[test]
    fn structural_validation_rejects_a_truncated_database() {
        let full = std::fs::read(fixture_city_db()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.mmdb");
        std::fs::write(&path, &full[..full.len() / 2]).unwrap();
        assert!(validate_structural(&path).is_err());
    }

    #[test]
    fn tmp_path_is_a_sibling_of_the_database() {
        let p = std::path::Path::new("/var/lib/edgion/geoip/db.mmdb");
        assert_eq!(
            tmp_path(p),
            std::path::PathBuf::from("/var/lib/edgion/geoip/db.mmdb.tmp")
        );
        assert_eq!(
            meta_path(p),
            std::path::PathBuf::from("/var/lib/edgion/geoip/db.mmdb.meta")
        );
    }

    /// Before the first success the backoff must be seconds, not the
    /// steady-state ten minutes — a cold-started pod is unprotected meanwhile.
    #[test]
    fn cold_start_backoff_escalates_from_one_second_capped_at_thirty() {
        assert_eq!(cold_start_backoff(0), Duration::from_secs(1));
        assert_eq!(cold_start_backoff(1), Duration::from_secs(2));
        assert_eq!(cold_start_backoff(4), Duration::from_secs(16));
        assert_eq!(cold_start_backoff(5), Duration::from_secs(30));
        assert_eq!(cold_start_backoff(50), Duration::from_secs(30));
    }

    /// Loopback is allowed under `cfg!(test)` — the same escape hatch
    /// `EgressResolver` uses, since the cycle tests below point `update_url` at
    /// `127.0.0.1`. Production builds do not carry this escape hatch.
    #[test]
    fn ip_literal_loopback_update_url_is_allowed_under_test_escape_hatch() {
        assert!(classify_literal_destination("http://127.0.0.1:9999/db.mmdb").is_ok());
    }

    #[test]
    fn ip_literal_link_local_metadata_address_is_rejected() {
        assert!(classify_literal_destination("http://169.254.169.254/db.mmdb").is_err());
    }

    #[test]
    fn hostname_update_url_is_not_classified_here() {
        // Hostname targets are out of scope for this literal check; DNS-time
        // egress policy is a separate concern from this task.
        assert!(classify_literal_destination("https://mirror.internal/db.mmdb").is_ok());
    }

    /// Serve one canned response on an ephemeral port; returns the base URL.
    async fn serve_once(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut throwaway = [0u8; 1024];
            let _ = sock.read(&mut throwaway).await;

            let status_line = match status {
                200 => "200 OK",
                304 => "304 Not Modified",
                other => panic!("serve_once: unsupported status {other} in test helper"),
            };
            let mut resp = format!("HTTP/1.1 {status_line}\r\nContent-Length: {}\r\n", body.len());
            for (k, v) in &headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            let _ = sock.write_all(resp.as_bytes()).await;
            if !body.is_empty() {
                let _ = sock.write_all(&body).await;
            }
            let _ = sock.shutdown().await;
        });
        format!("http://{addr}/")
    }

    fn updater_for(dir: &std::path::Path, url: String, max_db_bytes: u64) -> GeoIpUpdater {
        let cfg = GeoIpConfig {
            db_path: Some(dir.join("db.mmdb")),
            update_url: Some(url.into()),
            max_db_bytes,
            stall_timeout_sec: 5,
            download_deadline_sec: 30,
            ..Default::default()
        };
        // Production owns one process-global provider. Tests need independent
        // `'static` providers because updater cycles await network I/O and
        // must not mutate the global singleton across parallel tests.
        let provider = Box::leak(Box::new(GeoIpProvider::new()));
        GeoIpUpdater::new(cfg, provider).unwrap()
    }

    #[test]
    fn authorization_header_is_sensitive_before_storage_and_clone() {
        let secret = "Bearer private-geoip-token";
        let value = sensitive_authorization_header("GEOIP_AUTH", secret).unwrap();
        assert!(value.is_sensitive());

        let cloned = value.clone();
        assert!(cloned.is_sensitive(), "HeaderValue clone must preserve sensitivity");
        assert!(
            !format!("{cloned:?}").contains(secret),
            "structured HeaderValue Debug must not expose the credential"
        );

        let client = reqwest::Client::new();
        let (request, sent_validator) = build_request(
            &client,
            "http://example.invalid/geo.mmdb",
            Some(&cloned),
            &UpdateState::default(),
            true,
        );
        assert!(!sent_validator);
        let request = request.build().unwrap();
        assert!(request.headers().get(AUTHORIZATION).unwrap().is_sensitive());
    }

    #[test]
    fn invalid_authorization_header_error_names_only_the_environment_variable() {
        let raw = "Bearer private-geoip-token\ninvalid";
        let err = sensitive_authorization_header("GEOIP_AUTH", raw).unwrap_err();
        assert!(err.contains("GEOIP_AUTH"));
        assert!(!err.contains("private-geoip-token"));
        assert!(!err.contains(raw));
    }

    /// BLOCKER 5 regression: the precheck must require `max_db_bytes + 2x the
    /// current database size`, not `3x max_db_bytes`. A 70 MB database against
    /// a 512 MiB volume (comfortably 3x the *database*, per the docs) must not
    /// be treated as needing `3 * max_db_bytes` (768 MiB at the default cap) —
    /// that overshoot is exactly what wedged updates permanently for an
    /// operator who sized storage off the docs' own advice.
    #[test]
    fn free_space_formula_is_max_db_bytes_plus_twice_current_size_not_3x_max_db_bytes() {
        let max_db_bytes = 268_435_456u64; // 256 MiB default cap
        let current_db_size = 70 * 1024 * 1024u64; // 70 MB live database

        let needed = free_space_needed(max_db_bytes, current_db_size);
        let old_3x_formula = max_db_bytes.saturating_mul(3);

        assert_eq!(needed, max_db_bytes + 2 * current_db_size);
        assert!(
            needed < old_3x_formula,
            "the corrected formula must not overshoot the old 3x max_db_bytes formula \
             for a database smaller than the cap: needed={needed}, old_3x={old_3x_formula}"
        );

        // A 512 MiB volume is comfortably 3x this 70 MB database (the docs'
        // sizing rule), so the corrected formula must fit it while the old
        // 3x-max_db_bytes formula (768 MiB) would not.
        let volume_bytes = 512 * 1024 * 1024u64;
        assert!(needed <= volume_bytes, "corrected formula must fit a docs-sized volume");
        assert!(
            old_3x_formula > volume_bytes,
            "old formula must NOT fit — this is the bug the fix corrects"
        );
    }

    /// With no database on disk yet (first-ever start), there is nothing to
    /// double-hold, so the requirement falls back to `max_db_bytes` alone.
    #[test]
    fn free_space_formula_falls_back_to_max_db_bytes_with_no_existing_database() {
        assert_eq!(free_space_needed(268_435_456, 0), 268_435_456);
    }

    /// The free-space precheck failure must carry its own `disk` reason label,
    /// distinguishable from a real network `fetch` failure — collapsing the two
    /// (finding 6) sends an operator debugging the network for a storage
    /// problem.
    #[tokio::test]
    async fn insufficient_free_space_fails_with_disk_reason_not_fetch() {
        let dir = tempfile::tempdir().unwrap();
        // A `max_db_bytes` far larger than any real volume's free space forces
        // the precheck to fail before any request is issued.
        let url = "http://127.0.0.1:1/db.mmdb".to_string();
        let u = updater_for(dir.path(), url, u64::MAX / 2);
        let mut state = UpdateState::default();
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(
            err.reason(),
            "disk",
            "an oversized max_db_bytes must fail the precheck with reason=disk, not reason=fetch"
        );
    }

    /// BLOCKER 1 regression: `reqwest::Error`'s `Display` appends
    /// " for url (...)", and `url::Url`'s `Display` echoes userinfo and query
    /// verbatim. An `update_url` with an embedded credential must never survive
    /// into `UpdateError::detail()`, since that string is logged at ERROR level
    /// on every failed cycle (`run`, above). Port 1 is a deliberately
    /// unreachable host (nothing listens there), which forces a connect-stage
    /// `reqwest::Error` carrying the request URL.
    #[tokio::test]
    async fn fetch_error_detail_never_contains_the_update_url_or_its_credential() {
        let dir = tempfile::tempdir().unwrap();
        let secret_url = "http://svc:s3cr3t-token@127.0.0.1:1/db.mmdb?X-Amz-Signature=leaked".to_string();
        let u = updater_for(dir.path(), secret_url.clone(), 1 << 20);
        let mut state = UpdateState::default();
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(err.reason(), "fetch");
        assert!(
            !err.detail().contains("s3cr3t-token") && !err.detail().contains("X-Amz-Signature"),
            "UpdateError detail must never carry the update_url or its credential, got: {}",
            err.detail()
        );
        assert!(
            !err.detail().contains(&secret_url),
            "UpdateError detail must not embed the full update_url, got: {}",
            err.detail()
        );
    }

    #[tokio::test]
    async fn not_modified_with_usable_database_leaves_disk_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve_once(304, &[], Vec::new()).await;
        let u = updater_for(dir.path(), url, 1 << 20);
        std::fs::copy(fixture_city_db(), &u.db_path).unwrap();
        u.provider.load_from(&u.db_path).unwrap();
        let before = std::fs::read(&u.db_path).unwrap();
        let mut state = UpdateState {
            etag: Some("\"abc\"".into()),
            last_modified: None,
        };
        assert!(matches!(u.run_cycle(&mut state).await, Ok(CycleOutcome::NotModified)));
        assert_eq!(std::fs::read(dir.path().join("db.mmdb")).unwrap(), before);
        assert!(!dir.path().join("db.mmdb.tmp").exists());
        assert_eq!(state.etag.as_deref(), Some("\"abc\""));
    }

    #[test]
    fn invalid_not_modified_clears_validators_and_next_request_is_unconditional() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        let meta = meta_path(&db);
        let mut state = UpdateState {
            etag: Some("\"stale\"".into()),
            last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
        };
        state.persist(&meta);

        let err = handle_not_modified_response(&mut state, &meta, true, false).unwrap_err();
        assert_eq!(err.reason(), "fetch");
        assert!(err.requires_cold_backoff());
        assert!(state.etag.is_none());
        assert!(state.last_modified.is_none());

        // A valid empty sidecar is loadable and intentionally suppresses the
        // database-mtime fallback after local eligibility returns.
        std::fs::write(&db, b"regular database placeholder").unwrap();
        let reloaded = UpdateState::load(&meta, &db, true);
        assert!(reloaded.etag.is_none());
        assert!(reloaded.last_modified.is_none());

        let client = reqwest::Client::new();
        let (request, sent_validator) =
            build_request(&client, "http://example.invalid/geo.mmdb", None, &reloaded, true);
        assert!(!sent_validator);
        let request = request.build().unwrap();
        assert!(request.headers().get(IF_NONE_MATCH).is_none());
        assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
    }

    #[test]
    fn not_modified_without_a_sent_validator_is_invalid_even_with_usable_local_state() {
        let dir = tempfile::tempdir().unwrap();
        let meta = meta_path(&dir.path().join("db.mmdb"));
        let mut state = UpdateState::default();
        let err = handle_not_modified_response(&mut state, &meta, false, true).unwrap_err();
        assert_eq!(err.reason(), "fetch");
        assert!(err.requires_cold_backoff());
    }

    #[test]
    fn invalid_not_modified_forces_cold_backoff_even_with_an_active_provider() {
        let mut cold_attempt = 0;
        let delay = failure_sleep_base(
            true,
            UpdateError::InvalidNotModified("invalid 304".into()).requires_cold_backoff(),
            Duration::from_secs(600),
            &mut cold_attempt,
        );
        assert_eq!(delay, Duration::from_secs(1));
        assert_eq!(cold_attempt, 1);
    }

    /// The cap is enforced WHILE streaming; exceeding it must fail the cycle and
    /// leave no partial file, because a stale .tmp wedges every later cycle.
    #[tokio::test]
    async fn oversized_body_fails_and_removes_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve_once(200, &[], vec![0u8; 4096]).await;
        let u = updater_for(dir.path(), url, 16);
        let mut state = UpdateState::default();
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(err.reason(), "size_cap");
        assert!(!dir.path().join("db.mmdb.tmp").exists());
        assert!(!dir.path().join("db.mmdb").exists());
    }

    /// A gzip body written straight to .tmp would fail validation forever with an
    /// opaque error, so reject the encoding up front instead.
    #[tokio::test]
    async fn gzip_content_encoding_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve_once(200, &[("content-encoding", "gzip")], vec![0u8; 32]).await;
        let u = updater_for(dir.path(), url, 1 << 20);
        let mut state = UpdateState::default();
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(err.reason(), "encoding");
        assert!(!dir.path().join("db.mmdb.tmp").exists());
    }

    /// Half a broken database is worse than none.
    #[tokio::test]
    async fn failed_validation_never_renames() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve_once(200, &[], b"not an mmdb".to_vec()).await;
        let u = updater_for(dir.path(), url, 1 << 20);
        let mut state = UpdateState::default();
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(err.reason(), "validation");
        assert!(
            !dir.path().join("db.mmdb").exists(),
            "a rejected candidate must never be renamed into place"
        );
    }

    /// First-start `ensure_free_space` requires `max_db_bytes` on the temp
    /// volume. Keep this cap fixture-scale so a constrained host tmpfs (well
    /// under 1 GiB) can still exercise download, rename, swap, and `.meta`.
    /// Disk exhaustion is covered by
    /// `insufficient_free_space_fails_with_disk_reason_not_fetch`.
    #[tokio::test]
    async fn successful_cycle_renames_swaps_and_persists_meta() {
        let dir = tempfile::tempdir().unwrap();
        let body = std::fs::read(fixture_city_db()).unwrap();
        let url = serve_once(200, &[("etag", "\"v1\"")], body).await;
        let u = updater_for(dir.path(), url, 1 << 20);
        let mut state = UpdateState::default();
        assert!(matches!(u.run_cycle(&mut state).await, Ok(CycleOutcome::Swapped)));
        assert!(dir.path().join("db.mmdb").exists());
        assert!(!dir.path().join("db.mmdb.tmp").exists());
        assert!(dir.path().join("db.mmdb.meta").exists());
        assert_eq!(state.etag.as_deref(), Some("\"v1\""));
        assert!(u.provider.is_loaded());
    }

    /// A partial .tmp left by a crashed pod must not survive into the next cycle.
    #[tokio::test]
    async fn stale_tmp_is_removed_at_cycle_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("db.mmdb.tmp"), vec![0u8; 1024]).unwrap();
        let url = serve_once(304, &[], Vec::new()).await;
        let u = updater_for(dir.path(), url, 1 << 20);
        let mut state = UpdateState {
            etag: Some("\"abc\"".into()),
            last_modified: None,
        };
        let _ = u.run_cycle(&mut state).await;
        assert!(!dir.path().join("db.mmdb.tmp").exists());
    }

    /// `load` seeds `last_modified` from the database file's mtime when the
    /// `.meta` file is absent but the database already exists — the `emptyDir`
    /// case where the validator was lost with the volume.
    #[test]
    fn load_seeds_last_modified_from_db_mtime_when_meta_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        std::fs::write(&db, b"placeholder").unwrap();

        let state = UpdateState::load(&meta_path(&db), &db, true);
        assert!(state.etag.is_none());
        assert!(state.last_modified.is_some());
    }

    /// With neither `.meta` nor a database present (first-ever start), there is
    /// nothing to seed from.
    #[test]
    fn load_is_empty_when_neither_meta_nor_db_exist() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        let state = UpdateState::load(&meta_path(&db), &db, false);
        assert!(state.etag.is_none());
        assert!(state.last_modified.is_none());
    }

    /// `persist` followed by `load` round-trips the validator.
    #[test]
    fn persist_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        std::fs::write(&db, b"regular database placeholder").unwrap();
        let meta = meta_path(&db);
        let state = UpdateState {
            etag: Some("\"v2\"".into()),
            last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
        };
        state.persist(&meta);

        let reloaded = UpdateState::load(&meta, &db, true);
        assert_eq!(reloaded.etag, state.etag);
        assert_eq!(reloaded.last_modified, state.last_modified);
    }

    #[test]
    fn persisted_validators_require_both_regular_database_and_active_provider() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        let meta = meta_path(&db);
        let state = UpdateState {
            etag: Some("\"v3\"".into()),
            last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
        };
        state.persist(&meta);

        let missing_db = UpdateState::load(&meta, &db, true);
        assert!(missing_db.etag.is_none());
        assert!(missing_db.last_modified.is_none());

        std::fs::write(&db, b"regular database placeholder").unwrap();
        let restored_after_missing_db = UpdateState::load(&meta, &db, true);
        assert!(restored_after_missing_db.etag.is_none());
        assert!(restored_after_missing_db.last_modified.is_none());

        state.persist(&meta);
        let inactive_provider = UpdateState::load(&meta, &db, false);
        assert!(inactive_provider.etag.is_none());
        assert!(inactive_provider.last_modified.is_none());

        let restored_after_inactive_provider = UpdateState::load(&meta, &db, true);
        assert!(restored_after_inactive_provider.etag.is_none());
        assert!(restored_after_inactive_provider.last_modified.is_none());
    }

    #[test]
    fn validator_context_requires_a_regular_file_and_active_provider() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.mmdb");
        assert!(!validator_context_ready(&missing, true));

        assert!(!validator_context_ready(dir.path(), true));

        let db = dir.path().join("db.mmdb");
        std::fs::write(&db, b"regular database placeholder").unwrap();
        assert!(!validator_context_ready(&db, false));
        assert!(validator_context_ready(&db, true));
    }

    #[test]
    fn valid_empty_sidecar_suppresses_mtime_seed_when_context_is_usable() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        std::fs::copy(fixture_city_db(), &db).unwrap();
        let provider = Box::leak(Box::new(GeoIpProvider::new()));
        provider.load_from(&db).unwrap();

        let meta = meta_path(&db);
        UpdateState::default().persist(&meta);
        let state = UpdateState::load(&meta, &db, provider.is_loaded());
        assert!(state.etag.is_none());
        assert!(state.last_modified.is_none());
    }

    #[test]
    fn request_time_ineligibility_clears_memory_and_persists_empty_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.mmdb");
        let meta = meta_path(&db);
        let mut state = UpdateState {
            etag: Some("\"stale\"".into()),
            last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
        };
        state.persist(&meta);

        prepare_validator_state(&mut state, &meta, false);
        assert!(state.etag.is_none());
        assert!(state.last_modified.is_none());

        std::fs::write(&db, b"regular database placeholder").unwrap();
        let restored = UpdateState::load(&meta, &db, true);
        let client = reqwest::Client::new();
        let (request, sent_validator) =
            build_request(&client, "http://example.invalid/geo.mmdb", None, &restored, true);
        assert!(!sent_validator);
        let request = request.build().unwrap();
        assert!(request.headers().get(IF_NONE_MATCH).is_none());
        assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
    }

    /// A **hostname** `update_url` is exactly the case `classify_literal_destination`
    /// cannot see (it only classifies IP literals; `Host::Domain(_)` short-circuits
    /// to `Ok(())`). This proves the complementary half of the review fix: the
    /// client actually resolves and dials `localhost` through `EgressResolver`
    /// rather than through a bare `dns_resolver: None` reqwest default — if
    /// `GeoIpUpdater::new` had not wired a resolver, or had wired a broken one,
    /// this cycle would fail to connect at all.
    ///
    /// What this does NOT cover: asserting that a hostname resolving to a
    /// *blocked* address (private/link-local) is actually rejected. `loopback`
    /// is allowed under the same `cfg!(test)` escape hatch `EgressResolver`
    /// itself uses (see `ip_literal_loopback_update_url_is_allowed_under_test_escape_hatch`
    /// above), and `EgressResolver::new` hardcodes `ResolverSpec::System` with no
    /// test seam to inject a fake DNS answer for a non-loopback address, so a
    /// true positive-rejection test is not achievable at this layer without
    /// adding new resolver-injection plumbing that no other `EgressResolver`
    /// consumer in this codebase has either.
    #[tokio::test]
    async fn hostname_update_url_resolves_and_completes_a_cycle_through_egress_resolver() {
        let dir = tempfile::tempdir().unwrap();
        let addr_url = serve_once(200, &[("etag", "\"v1\"")], b"not an mmdb".to_vec()).await;
        // serve_once binds 127.0.0.1:<port>; swap in a hostname so the request
        // goes through DNS resolution (and therefore through `EgressResolver`)
        // instead of `classify_literal_destination`'s IP-literal path.
        let hostname_url = addr_url.replacen("127.0.0.1", "localhost", 1);

        let u = updater_for(dir.path(), hostname_url, 1 << 20);
        let mut state = UpdateState::default();
        // The body is not a valid mmdb, so the cycle fails at structural
        // validation — but only after the resolver connected and the response
        // was fetched, which is exactly what proves EgressResolver is wired in.
        let err = u.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(
            err.reason(),
            "validation",
            "a fetch/connect-stage failure would indicate the resolver never connected"
        );
    }
}
