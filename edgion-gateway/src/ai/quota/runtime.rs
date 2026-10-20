use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{AiQuotaRequest, QuotaApplyResult, QuotaFuture, QuotaSink};
use crate::link_sys::get_redis_client;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::plugins::common::redis_rate_limiter::{
    build_cumulative_quota_keys, CumulativeQuotaMode, CumulativeQuotaResult, CumulativeQuotaStatus, RedisRateLimiter,
};
use crate::plugins::http::common::{resolve_rate_limit_key, RateLimitKey};
use crate::plugins::runtime::{PluginLog, PluginSession};
use edgion_resources::resources::edgion_plugins::{
    AiTokenQuotaConfig, AiTokenQuotaOnExceeded, AiTokenQuotaScope, OnRedisFailure,
};

const KEY_PREFIX: &str = "edgion:aiq:";
const RECONCILIATION_GRACE: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct CompiledLimit {
    tokens: u64,
    window_ms: u128,
    scope: AiTokenQuotaScope,
}

/// Cold-compiled, immutable quota configuration owned by one AiProxy entry.
pub struct CompiledAiQuotaPlan {
    redis_ref: String,
    key: Vec<edgion_resources::common::KeyGet>,
    on_key_miss: edgion_resources::resources::edgion_plugins::OnKeyMiss,
    on_redis_failure: OnRedisFailure,
    limits: Vec<CompiledLimit>,
    flush_tokens: u64,
    flush_interval: Duration,
    request_timeout: Duration,
    on_exceeded: QuotaExceededAction,
    entry_scope: String,
    core: Arc<RedisRateLimiter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuotaExceededAction {
    Reject,
    Throttle { max_delay: Duration },
    Downgrade { model: String },
}

pub enum QuotaAdmission {
    Ready(AiQuotaRequest),
    /// Admission was intentionally skipped by `onKeyMiss: allow`.
    Bypass,
    /// Redis failed and `onRedisFailure: allow` explicitly permitted traffic.
    DependencyBypass,
    IdentityMissing,
    Exhausted(Duration),
    DependencyFailed,
}

trait QuotaAdmissionSource {
    async fn admit_once(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        logical_model: &str,
        maximum_request_duration: Duration,
    ) -> QuotaAdmission;
}

impl CompiledAiQuotaPlan {
    pub fn compile(config: &AiTokenQuotaConfig, entry_scope: &str) -> Option<Self> {
        let windows = config.parsed_windows().ok()?;
        let on_exceeded = match &config.on_exceeded {
            AiTokenQuotaOnExceeded::Reject => QuotaExceededAction::Reject,
            AiTokenQuotaOnExceeded::Throttle { .. } => QuotaExceededAction::Throttle {
                max_delay: config.on_exceeded.parsed_max_delay().ok()??,
            },
            AiTokenQuotaOnExceeded::Downgrade { model } => QuotaExceededAction::Downgrade { model: model.clone() },
        };
        Some(Self {
            redis_ref: config.redis_ref.clone(),
            key: config.key.clone(),
            on_key_miss: config.on_key_miss.clone(),
            on_redis_failure: config.on_redis_failure.clone(),
            limits: config
                .limits
                .iter()
                .zip(windows)
                .map(|(limit, window)| CompiledLimit {
                    tokens: limit.tokens,
                    window_ms: window.as_millis().max(1),
                    scope: limit.scope,
                })
                .collect(),
            flush_tokens: config.flush_tokens,
            flush_interval: config.parsed_flush_interval().ok()?,
            request_timeout: config.parsed_request_timeout().ok()?,
            on_exceeded,
            entry_scope: entry_scope.to_string(),
            core: Arc::new(RedisRateLimiter::new()),
        })
    }

    pub fn on_exceeded(&self) -> &QuotaExceededAction {
        &self.on_exceeded
    }

    pub async fn admit(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        logical_model: &str,
        maximum_request_duration: Duration,
    ) -> QuotaAdmission {
        admit_with_one_shot_throttle(
            self,
            &self.on_exceeded,
            session,
            log,
            logical_model,
            maximum_request_duration,
        )
        .await
    }

    async fn admit_once(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        logical_model: &str,
        maximum_request_duration: Duration,
    ) -> QuotaAdmission {
        let identity = match resolve_rate_limit_key(session, &self.key, None, &self.on_key_miss, log) {
            RateLimitKey::Resolved(value) => value,
            RateLimitKey::Allow => return QuotaAdmission::Bypass,
            RateLimitKey::Deny => return QuotaAdmission::IdentityMissing,
        };
        let Some(client) = get_redis_client(&self.redis_ref) else {
            return self.redis_failure();
        };
        if !client.healthy() {
            return self.redis_failure();
        }

        let mut request_id_buffer = uuid::Uuid::encode_buffer();
        let request_id = uuid::Uuid::new_v4().simple().encode_lower(&mut request_id_buffer);
        let targets = build_targets(
            &self.entry_scope,
            &identity,
            logical_model,
            request_id,
            &self.limits,
            maximum_request_duration,
            self.request_timeout,
            current_unix_millis(),
        );
        // build_targets assigns the same active lease TTL to every limit.
        let lease_refresh_interval = targets
            .first()
            .map(|target| lease_refresh_interval(target.active_lease_ttl))
            .expect("validated quota plan has at least one limit");
        drop(identity);
        let sink = Arc::new(RedisQuotaSink {
            client,
            core: Arc::clone(&self.core),
            targets,
            request_timeout: self.request_timeout,
        });
        let admission = tokio::time::timeout(self.request_timeout, sink.admit_all()).await;
        match admission {
            Ok(Ok(None)) => QuotaAdmission::Ready(AiQuotaRequest::start(
                sink,
                self.flush_tokens,
                self.flush_interval,
                lease_refresh_interval,
                self.request_timeout
                    .saturating_mul(2)
                    .saturating_add(Duration::from_millis(50)),
            )),
            Ok(Ok(Some(retry_after))) => QuotaAdmission::Exhausted(retry_after),
            Ok(Err(())) | Err(_) => self.redis_failure(),
        }
    }

    fn redis_failure(&self) -> QuotaAdmission {
        match self.on_redis_failure {
            OnRedisFailure::Allow => QuotaAdmission::DependencyBypass,
            OnRedisFailure::Deny => QuotaAdmission::DependencyFailed,
        }
    }
}

impl QuotaAdmissionSource for CompiledAiQuotaPlan {
    async fn admit_once(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        logical_model: &str,
        maximum_request_duration: Duration,
    ) -> QuotaAdmission {
        CompiledAiQuotaPlan::admit_once(self, session, log, logical_model, maximum_request_duration).await
    }
}

async fn admit_with_one_shot_throttle<S: QuotaAdmissionSource + ?Sized>(
    source: &S,
    action: &QuotaExceededAction,
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    logical_model: &str,
    maximum_request_duration: Duration,
) -> QuotaAdmission {
    let first = source
        .admit_once(session, log, logical_model, maximum_request_duration)
        .await;
    let QuotaAdmission::Exhausted(retry_after) = first else {
        return first;
    };
    let QuotaExceededAction::Throttle { max_delay } = action else {
        return QuotaAdmission::Exhausted(retry_after);
    };

    let action_budget = throttle_action_budget(*max_delay, remaining_request_timeout(session));
    let action_started = Instant::now();
    let delay = retry_after.min(action_budget);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let remaining_action_budget = remaining_throttle_budget(action_budget, action_started.elapsed());
    if remaining_action_budget.is_zero() || session.ctx().request_timeout_exceeded() {
        return QuotaAdmission::Exhausted(retry_after);
    }
    match tokio::time::timeout(
        remaining_action_budget,
        source.admit_once(session, log, logical_model, maximum_request_duration),
    )
    .await
    {
        Ok(admission) => admission,
        Err(_) => QuotaAdmission::Exhausted(retry_after),
    }
}

fn remaining_request_timeout(session: &dyn PluginSession) -> Option<Duration> {
    let ctx = session.ctx();
    ctx.request_timeout()
        .map(|timeout| timeout.saturating_sub(ctx.start_time.elapsed()))
}

fn throttle_action_budget(max_delay: Duration, remaining_request_timeout: Option<Duration>) -> Duration {
    remaining_request_timeout.map_or(max_delay, |remaining| max_delay.min(remaining))
}

fn remaining_throttle_budget(action_budget: Duration, elapsed: Duration) -> Duration {
    action_budget.saturating_sub(elapsed)
}

struct QuotaTarget {
    bucket_key: String,
    request_key: String,
    limit: u64,
    bucket_retention_deadline: Instant,
    active_lease_ttl: Duration,
    final_marker_ttl: Duration,
    retry_after: Duration,
}

struct RedisQuotaSink {
    client: Arc<RedisLinkClient>,
    core: Arc<RedisRateLimiter>,
    targets: Vec<QuotaTarget>,
    request_timeout: Duration,
}

fn project_apply_result(limit: u64, result: anyhow::Result<CumulativeQuotaResult>) -> Result<bool, ()> {
    match result.map(|value| (value.status, value.used)) {
        Ok((CumulativeQuotaStatus::Applied, used))
        | Ok((CumulativeQuotaStatus::Duplicate, used))
        | Ok((CumulativeQuotaStatus::Stale, used)) => Ok(used > limit),
        _ => Err(()),
    }
}

fn summarize_apply_results(results: impl IntoIterator<Item = Result<bool, ()>>) -> Result<QuotaApplyResult, ()> {
    let mut over_limit = false;
    let mut failed = false;
    for result in results {
        match result {
            Ok(exceeded) => over_limit |= exceeded,
            Err(()) => failed = true,
        }
    }
    if failed {
        Err(())
    } else {
        Ok(QuotaApplyResult { over_limit })
    }
}

impl RedisQuotaSink {
    async fn admit_all(&self) -> Result<Option<Duration>, ()> {
        let mut retry_after = None;
        for target in &self.targets {
            let result = self
                .core
                .cumulative_quota_admission(&self.client, &target.bucket_key, target.limit)
                .await
                .map_err(|_| ())?;
            if result.status == CumulativeQuotaStatus::Denied {
                retry_after =
                    Some(retry_after.map_or(target.retry_after, |current: Duration| current.max(target.retry_after)));
                continue;
            }
            if result.status != CumulativeQuotaStatus::Admitted {
                return Err(());
            }
        }
        Ok(retry_after)
    }

    async fn apply_all(&self, revision: u64, total: u64, final_update: bool) -> Result<QuotaApplyResult, ()> {
        let operation = async {
            let results = futures::future::join_all(self.targets.iter().map(|target| async move {
                let now = Instant::now();
                let marker_ttl = if final_update {
                    target.final_marker_ttl
                } else {
                    target.active_lease_ttl
                };
                let bucket_ttl = remaining_ttl(target.bucket_retention_deadline, now)
                    .unwrap_or_default()
                    .max(marker_ttl);
                let result = self
                    .core
                    .apply_cumulative_quota(
                        &self.client,
                        &target.bucket_key,
                        &target.request_key,
                        revision,
                        total,
                        final_update,
                        target.limit,
                        bucket_ttl,
                        marker_ttl,
                        CumulativeQuotaMode::Force,
                    )
                    .await;
                project_apply_result(target.limit, result)
            }))
            .await;
            summarize_apply_results(results)
        };
        tokio::time::timeout(self.request_timeout, operation)
            .await
            .map_err(|_| ())?
    }
}

impl QuotaSink for RedisQuotaSink {
    fn apply_cumulative(&self, revision: u64, total: u64, final_update: bool) -> QuotaFuture<'_> {
        Box::pin(self.apply_all(revision, total, final_update))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_targets(
    entry_scope: &str,
    identity: &str,
    logical_model: &str,
    request_id: &str,
    limits: &[CompiledLimit],
    maximum_request_duration: Duration,
    request_timeout: Duration,
    now_ms: u128,
) -> Vec<QuotaTarget> {
    let active_lease_ttl = maximum_request_duration
        .saturating_add(request_timeout)
        .saturating_add(RECONCILIATION_GRACE);
    let final_marker_ttl = request_timeout.saturating_add(RECONCILIATION_GRACE);
    let now_instant = Instant::now();
    let mut quota_identity = Vec::new();

    limits
        .iter()
        .map(|limit| {
            let window_ms = limit.window_ms;
            let window_id = now_ms / window_ms;
            let remaining_ms = window_ms - now_ms % window_ms;
            let bucket_retention_deadline = now_instant
                .checked_add(duration_from_millis_saturating(remaining_ms).saturating_add(RECONCILIATION_GRACE))
                .unwrap_or(now_instant);
            quota_identity.clear();
            quota_identity.reserve(entry_scope.len() + identity.len() + logical_model.len() + 32);
            append_identity_part(&mut quota_identity, entry_scope.as_bytes());
            append_identity_part(&mut quota_identity, identity.as_bytes());
            append_identity_part(&mut quota_identity, &window_ms.to_be_bytes());
            append_identity_part(&mut quota_identity, quota_scope_identity(limit.scope));
            if limit.scope == AiTokenQuotaScope::IdentityModel {
                append_identity_part(&mut quota_identity, logical_model.as_bytes());
            }
            let keys =
                build_cumulative_quota_keys(KEY_PREFIX, &quota_identity, window_id as u64, request_id.as_bytes())
                    .expect("validated fixed prefix and non-empty quota identities");
            QuotaTarget {
                bucket_key: keys.bucket,
                request_key: keys.marker,
                limit: limit.tokens,
                bucket_retention_deadline,
                active_lease_ttl,
                final_marker_ttl,
                retry_after: duration_from_millis_saturating(remaining_ms),
            }
        })
        .collect()
}

fn current_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn append_identity_part(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

const fn quota_scope_identity(scope: AiTokenQuotaScope) -> &'static [u8] {
    match scope {
        AiTokenQuotaScope::Identity => b"identity",
        AiTokenQuotaScope::IdentityModel => b"identity-model",
    }
}

fn duration_from_millis_saturating(milliseconds: u128) -> Duration {
    Duration::from_millis(u64::try_from(milliseconds).unwrap_or(u64::MAX))
}

fn remaining_ttl(deadline: Instant, now: Instant) -> Option<Duration> {
    let ttl = deadline.checked_duration_since(now)?;
    (ttl >= Duration::from_millis(1)).then_some(ttl)
}

fn lease_refresh_interval(lease_ttl: Duration) -> Duration {
    const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
    const MIN_EXPIRY_MARGIN: Duration = Duration::from_millis(1);

    let half = lease_ttl / 2;
    half.max(MIN_REFRESH_INTERVAL)
        .min(lease_ttl.saturating_sub(MIN_EXPIRY_MARGIN))
        .max(MIN_EXPIRY_MARGIN)
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/quota/runtime_test.rs"]
mod tests;
