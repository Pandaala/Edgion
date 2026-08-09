use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{AiQuotaRequest, QuotaApplyResult, QuotaFuture, QuotaSink};
use crate::link_sys::get_redis_client;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::plugins::common::redis_rate_limiter::{
    build_cumulative_quota_keys, CumulativeQuotaMode, CumulativeQuotaStatus, RedisRateLimiter,
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
    window: Duration,
    scope: AiTokenQuotaScope,
}

/// Cold-compiled, immutable quota configuration owned by one AiProxy entry.
pub struct CompiledAiQuotaPlan {
    redis_ref: String,
    key: Vec<edgion_resources::common::KeyGet>,
    on_missing_key: edgion_resources::resources::edgion_plugins::OnMissingKey,
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
    /// Admission was intentionally skipped by `onMissingKey: allow`.
    Bypass,
    /// Redis failed and `onRedisFailure: allow` explicitly permitted traffic.
    DependencyBypass,
    IdentityMissing,
    Exhausted(Duration),
    DependencyFailed,
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
            on_missing_key: config.on_missing_key.clone(),
            on_redis_failure: config.on_redis_failure.clone(),
            limits: config
                .limits
                .iter()
                .zip(windows)
                .map(|(limit, window)| CompiledLimit {
                    tokens: limit.tokens,
                    window,
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
        let identity = match resolve_rate_limit_key(session, &self.key, None, &self.on_missing_key, log).await {
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

        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let targets = build_targets(
            &self.entry_scope,
            &identity,
            logical_model,
            &request_id,
            &self.limits,
            maximum_request_duration,
            self.request_timeout,
        );
        let lease_refresh_interval = targets
            .iter()
            .map(|target| lease_refresh_interval(target.active_lease_ttl))
            .min()
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
            let mut over_limit = false;
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
                (target.limit, result)
            }))
            .await;
            let mut failed = false;
            for (limit, result) in results {
                match result.map(|value| (value.status, value.used)) {
                    Ok((CumulativeQuotaStatus::Applied, used))
                    | Ok((CumulativeQuotaStatus::Duplicate, used))
                    | Ok((CumulativeQuotaStatus::Stale, used)) => {
                        over_limit |= used > limit;
                    }
                    _ => failed = true,
                }
            }
            if failed {
                Err(())
            } else {
                Ok(QuotaApplyResult { over_limit })
            }
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

fn build_targets(
    entry_scope: &str,
    identity: &str,
    logical_model: &str,
    request_id: &str,
    limits: &[CompiledLimit],
    maximum_request_duration: Duration,
    request_timeout: Duration,
) -> Vec<QuotaTarget> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let active_lease_ttl = maximum_request_duration
        .saturating_add(request_timeout)
        .saturating_add(RECONCILIATION_GRACE);
    let final_marker_ttl = request_timeout.saturating_add(RECONCILIATION_GRACE);
    let now_instant = Instant::now();

    limits
        .iter()
        .map(|limit| {
            let window_ms = limit.window.as_millis().max(1);
            let window_id = now_ms / window_ms;
            let remaining_ms = window_ms - now_ms % window_ms;
            let bucket_retention_deadline = now_instant
                .checked_add(duration_from_millis_saturating(remaining_ms).saturating_add(RECONCILIATION_GRACE))
                .unwrap_or(now_instant);
            let mut quota_identity = Vec::with_capacity(entry_scope.len() + identity.len() + logical_model.len() + 32);
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
mod tests {
    use super::*;
    use serde_json::json;

    fn quota_config(on_exceeded: serde_json::Value) -> AiTokenQuotaConfig {
        serde_json::from_value(json!({
            "redisRef": "tenant/redis",
            "key": [],
            "limits": [{
                "scope": "identityModel",
                "tokens": 100,
                "window": "1m"
            }],
            "onExceeded": on_exceeded
        }))
        .unwrap()
    }

    #[test]
    fn quota_actions_are_frozen_during_compile() {
        let throttle =
            CompiledAiQuotaPlan::compile(&quota_config(json!({"type": "Throttle", "maxDelay": "250ms"})), "scope")
                .unwrap();
        assert_eq!(
            throttle.on_exceeded(),
            &QuotaExceededAction::Throttle {
                max_delay: Duration::from_millis(250)
            }
        );

        let downgrade =
            CompiledAiQuotaPlan::compile(&quota_config(json!({"type": "Downgrade", "model": "economy"})), "scope")
                .unwrap();
        assert_eq!(
            downgrade.on_exceeded(),
            &QuotaExceededAction::Downgrade {
                model: "economy".to_string()
            }
        );
    }

    #[test]
    fn quota_keys_hash_identity_and_share_one_cluster_slot() {
        let limits = vec![CompiledLimit {
            tokens: 100,
            window: Duration::from_secs(60),
            scope: AiTokenQuotaScope::IdentityModel,
        }];
        let targets = build_targets(
            "tenant/plugins/#0",
            "sensitive-consumer",
            "model-a",
            "request1",
            &limits,
            Duration::from_secs(300),
            Duration::from_millis(500),
        );
        assert_eq!(targets.len(), 1);
        assert!(!targets[0].bucket_key.contains("sensitive-consumer"));
        let bucket_slot = targets[0].bucket_key.split(['{', '}']).nth(1).unwrap();
        let request_slot = targets[0].request_key.split(['{', '}']).nth(1).unwrap();
        assert_eq!(bucket_slot, request_slot);
    }

    #[test]
    fn identity_model_scope_separates_requested_names_but_identity_scope_does_not() {
        let identity_limits = vec![CompiledLimit {
            tokens: 100,
            window: Duration::from_secs(60),
            scope: AiTokenQuotaScope::Identity,
        }];
        let model_limits = vec![CompiledLimit {
            scope: AiTokenQuotaScope::IdentityModel,
            ..identity_limits[0].clone()
        }];
        let identity_a = build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            &identity_limits,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        let identity_b = build_targets(
            "scope",
            "consumer",
            "model-b",
            "r1",
            &identity_limits,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        assert_eq!(identity_a[0].bucket_key, identity_b[0].bucket_key);
        let model_a = build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            &model_limits,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        let model_b = build_targets(
            "scope",
            "consumer",
            "model-b",
            "r1",
            &model_limits,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        assert_ne!(model_a[0].bucket_key, model_b[0].bucket_key);
    }

    #[test]
    fn quota_ttl_counts_down_from_one_absolute_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        assert_eq!(remaining_ttl(deadline, now), Some(Duration::from_secs(10)));
        assert_eq!(
            remaining_ttl(deadline, now + Duration::from_secs(4)),
            Some(Duration::from_secs(6))
        );
        assert_eq!(remaining_ttl(deadline, now + Duration::from_secs(11)), None);
    }

    #[test]
    fn quota_policy_identity_is_stable_across_reorder_and_limit_changes() {
        let first = CompiledLimit {
            tokens: 100,
            window: Duration::from_secs(60),
            scope: AiTokenQuotaScope::Identity,
        };
        let second = CompiledLimit {
            tokens: 200,
            window: Duration::from_secs(3_600),
            scope: AiTokenQuotaScope::IdentityModel,
        };
        let original = build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            &[first.clone(), second.clone()],
            Duration::from_secs(300),
            Duration::from_millis(500),
        );
        let mut changed_limit = first.clone();
        changed_limit.tokens = 999;
        let reordered = build_targets(
            "scope",
            "consumer",
            "model-a",
            "r1",
            &[second, changed_limit],
            Duration::from_secs(300),
            Duration::from_millis(500),
        );

        assert_eq!(original[0].bucket_key, reordered[1].bucket_key);
        assert_eq!(original[0].request_key, reordered[1].request_key);
        assert_eq!(original[1].bucket_key, reordered[0].bucket_key);
        assert_ne!(original[0].bucket_key, original[1].bucket_key);
    }

    #[test]
    fn lease_refresh_is_sparse_and_stays_before_expiry() {
        assert_eq!(lease_refresh_interval(Duration::from_secs(10)), Duration::from_secs(5));
        assert_eq!(
            lease_refresh_interval(Duration::from_secs(1)),
            Duration::from_millis(999)
        );
        assert_eq!(
            lease_refresh_interval(Duration::from_millis(10)),
            Duration::from_millis(9)
        );
    }
}
