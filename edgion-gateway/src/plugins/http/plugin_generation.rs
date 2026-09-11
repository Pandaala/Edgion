//! Immutable HTTP plugin source and its bounded, demand-owned constructor cache.

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::{duplicate_alias_error, EdgionPlugins};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily, PluginPolicyAction};

use super::plugin_store::{compile_plugins_with_cache, CompiledPlugins};
use crate::plugins::runtime::entry_bundle::{EntryCache, EntryDemand, EntrySlot, EntryStage};

/// Slots never cross this immutable source boundary. Policy edits reuse this
/// owner; source replacement creates another owner, even for the same key.
pub(crate) struct PluginGeneration {
    source: EdgionPlugins,
    alias_errors: Vec<(EntrySlot, String)>,
    cache: EntryCache,
}

impl PluginGeneration {
    pub fn new(mut source: EdgionPlugins) -> Self {
        let mut cache = EntryCache::default();
        cache.prepare_request_outcomes(source.spec.request_plugins.as_deref().unwrap_or_default(), 0);
        let mut alias_errors = Vec::new();
        macro_rules! prepare {
            ($entries:expr, $stage:ident, $label:literal) => {
                if let Some(entries) = $entries.as_mut() {
                    let mut aliases = HashSet::new();
                    for (index, entry) in entries.iter_mut().enumerate() {
                        if !entry.is_enabled() {
                            continue;
                        }
                        let error = entry
                            .validate_alias()
                            .or_else(|| duplicate_alias_error(&mut aliases, entry.alias.as_deref()));
                        if let Some(error) = error {
                            alias_errors.push((
                                EntrySlot {
                                    stage: EntryStage::$stage,
                                    index,
                                },
                                format!("{}[{}] ({}): {}", $label, index, entry.plugin.type_name(), error),
                            ));
                            entry.alias = None;
                        }
                    }
                }
            };
        }
        prepare!(source.spec.request_plugins, Request, "requestPlugins");
        prepare!(
            source.spec.upstream_response_filter_plugins,
            ResponseFilter,
            "upstreamResponseFilterPlugins"
        );
        prepare!(
            source.spec.upstream_response_body_filter_plugins,
            ResponseBody,
            "upstreamResponseBodyFilterPlugins"
        );
        prepare!(
            source.spec.upstream_response_plugins,
            Response,
            "upstreamResponsePlugins"
        );
        Self {
            source,
            alias_errors,
            cache,
        }
    }

    pub fn source(&self) -> &EdgionPlugins {
        &self.source
    }

    /// Preserve declaration order so the first current-source Block determines
    /// the response before restrictions found only in a retained candidate.
    pub fn policy_actions(&self, policy: Option<&CompiledPluginPolicy>) -> Vec<(&'static str, PluginPolicyAction)> {
        let mut actions = Vec::new();
        macro_rules! collect {
            ($entries:expr) => {
                for entry in $entries.iter().flatten().filter(|entry| entry.is_enabled()) {
                    if let Some(action) =
                        policy.and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.type_name()))
                    {
                        actions.push((entry.plugin.type_name(), action));
                    }
                }
            };
        }
        collect!(self.source.spec.request_plugins);
        collect!(self.source.spec.upstream_response_filter_plugins);
        collect!(self.source.spec.upstream_response_body_filter_plugins);
        collect!(self.source.spec.upstream_response_plugins);
        actions
    }

    /// Demand is the union of these sets across scopes that currently attach
    /// this resource. Computing a mask never constructs an executable plugin.
    pub fn demand(&self, policy: Option<&CompiledPluginPolicy>) -> EntryDemand {
        let mut needed = EntryDemand::new();
        macro_rules! collect {
            ($entries:expr, $stage:ident) => {
                for (index, entry) in $entries.iter().flatten().enumerate() {
                    if entry.is_enabled()
                        && policy
                            .and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.type_name()))
                            .is_none()
                    {
                        needed.insert(EntrySlot {
                            stage: EntryStage::$stage,
                            index,
                        });
                    }
                }
            };
        }
        collect!(self.source.spec.request_plugins, Request);
        collect!(self.source.spec.upstream_response_filter_plugins, ResponseFilter);
        collect!(self.source.spec.upstream_response_body_filter_plugins, ResponseBody);
        collect!(self.source.spec.upstream_response_plugins, Response);
        if self.source.spec.waf.is_some() {
            needed.insert(EntrySlot {
                stage: EntryStage::Waf,
                index: 0,
            });
        }
        needed
    }

    /// Construct only permitted slots and assemble a new effective view.
    /// Block/Bypass gates are recorded separately from transport entry fields.
    pub fn compile(&mut self, policy: Option<&CompiledPluginPolicy>) -> Arc<CompiledPlugins> {
        self.compile_with_additional_actions(policy, &[])
    }

    /// A retained executable source must also enforce restrictions declared by
    /// the latest source, including entries that the retained source never had.
    pub fn compile_with_additional_actions(
        &mut self,
        policy: Option<&CompiledPluginPolicy>,
        extra_actions: &[(&'static str, PluginPolicyAction)],
    ) -> Arc<CompiledPlugins> {
        let needed = self.demand(policy);
        let mut effective = self.source.clone();
        let mut actions = extra_actions.to_vec();
        actions.extend(self.policy_actions(policy));
        let mut bypassed = HashSet::new();
        actions.retain(|(name, action)| *action != PluginPolicyAction::Bypass || bypassed.insert(*name));
        macro_rules! mask {
            ($entries:expr) => {
                for entry in $entries.iter_mut().flatten() {
                    if !entry.is_enabled() {
                        continue;
                    }
                    if policy
                        .and_then(|p| p.action_for(PluginFamily::Http, entry.plugin.type_name()))
                        .is_some()
                    {
                        entry.enable = false;
                    }
                }
            };
        }
        mask!(effective.spec.request_plugins);
        mask!(effective.spec.upstream_response_filter_plugins);
        mask!(effective.spec.upstream_response_body_filter_plugins);
        mask!(effective.spec.upstream_response_plugins);
        let errors = self
            .alias_errors
            .iter()
            .filter(|(slot, _)| needed.contains(slot))
            .map(|(_, error)| error.clone())
            .collect();
        compile_plugins_with_cache(&effective, &mut self.cache, &actions, errors)
    }

    /// Clear publisher-owned strong references after all active scopes have
    /// contributed demand. In-flight runtime Arcs retain their own filters.
    pub fn release_unneeded_entries(&mut self, needed: &EntryDemand) {
        self.cache.release_unneeded_entries(needed);
    }

    pub fn cached_entry_count(&self) -> usize {
        self.cache.len()
    }

    pub fn cached_entry_keys(&self) -> impl Iterator<Item = EntrySlot> + '_ {
        self.cache.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::entry_bundle::EntryOutcome;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::{PluginLog, RequestFilter};
    use crate::EdgionHttpContext;
    use edgion_resources::filters::PluginRunningResult;
    use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PluginPolicyEntryConfig, PolicyActionKind};
    use serde_json::{json, Value};

    fn resource(spec: Value) -> EdgionPlugins {
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionPlugins",
            "metadata": {"namespace": "apps", "name": "shared"},
            "spec": spec,
        }))
        .unwrap()
    }

    fn restriction(names: &[&str], action: PolicyActionKind) -> CompiledPluginPolicy {
        PluginPolicyConfig {
            deny: names
                .iter()
                .map(|name| PluginPolicyEntryConfig {
                    name: format!("http/{name}"),
                    action: Some(action),
                    block_status: None,
                })
                .collect(),
            ..Default::default()
        }
        .compile()
        .unwrap()
    }

    fn request_filter(view: &CompiledPlugins, name: &str) -> Arc<dyn RequestFilter> {
        view.plugin_runtime
            .request_plugins_iter()
            .find(|filter| filter.name() == name)
            .unwrap()
            .clone()
    }

    #[test]
    fn rate_limit_local_is_lazy_shared_and_released_after_the_last_scope() {
        let source = resource(json!({"requestPlugins": [{"type": "RateLimitLocal", "config": {"rate": 100}}]}));
        let mut generation = PluginGeneration::new(source.clone());
        let blocked = restriction(&["RateLimitLocal"], PolicyActionKind::Block);
        let restricted_view = generation.compile(Some(&blocked));
        assert!(restricted_view.is_activatable);
        assert_eq!(generation.cached_entry_count(), 0);
        assert!(restricted_view
            .plugin_runtime
            .request_plugins_iter()
            .all(|filter| filter.name() != "RateLimitLocal"));

        let first = generation.compile(None);
        let second = generation.compile(None);
        assert!(first.is_activatable, "{:?}", first.preparse_errors);
        let first_filter = request_filter(&first, "RateLimitLocal");
        let second_filter = request_filter(&second, "RateLimitLocal");
        assert!(Arc::ptr_eq(&first_filter, &second_filter));
        let weak = Arc::downgrade(&first_filter);

        // One allowed scope still contributes demand after another is revoked.
        let allowed_demand = generation.demand(None);
        generation.release_unneeded_entries(&allowed_demand);
        assert_eq!(generation.cached_entry_count(), 1);
        generation.release_unneeded_entries(&generation.demand(Some(&blocked)));
        assert_eq!(generation.cached_entry_count(), 0);
        drop(first_filter);
        drop(second_filter);
        drop(first);
        assert!(weak.upgrade().is_some(), "an in-flight view retains its instance");
        drop(second);
        assert!(
            weak.upgrade().is_none(),
            "revoked cache slots must release their instance"
        );

        let resumed = generation.compile(None);
        let resumed_filter = request_filter(&resumed, "RateLimitLocal");
        assert!(!std::ptr::addr_eq(weak.as_ptr(), Arc::as_ptr(&resumed_filter)));
        let mut replacement = PluginGeneration::new(source);
        let replaced = replacement.compile(None);
        assert!(!Arc::ptr_eq(
            &resumed_filter,
            &request_filter(&replaced, "RateLimitLocal")
        ));
        assert_eq!(replaced.plugin_runtime.rate_limit_local_outcome_slots(), 0);
    }

    #[tokio::test]
    async fn rate_limit_local_outcomes_are_stable_across_policy_construction_order() {
        use pingora_proxy::Session;
        use tokio::io::AsyncWriteExt;

        let source = resource(json!({"requestPlugins": [
            {"type": "RateLimitLocal", "alias": "local", "config": {
                "rate": 100, "interval": "1h", "key": [],
                "onLimitExceeded": "Continue", "showLimitHeaders": false
            }},
            {"type": "RateLimitRedis", "conditions": {"run": {"allOf": [
                {"type": "rateLimitExceeded", "pluginAlias": "local"}
            ]}}, "config": {
                "redisRef": "apps/intentionally-absent", "key": [],
                "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],
                "onRedisFailure": "Deny", "showLimitHeaders": false
            }}
        ]}));
        let consumer_bypass = restriction(&["RateLimitRedis"], PolicyActionKind::Bypass);
        let producer_bypass = restriction(&["RateLimitLocal"], PolicyActionKind::Bypass);
        for bypass_first in [true, false] {
            let mut generation = PluginGeneration::new(source.clone());
            let first = generation.compile(bypass_first.then_some(&consumer_bypass));
            let second = generation.compile((!bypass_first).then_some(&consumer_bypass));
            assert!(Arc::ptr_eq(
                &request_filter(&first, "RateLimitLocal"),
                &request_filter(&second, "RateLimitLocal")
            ));
            for view in [&first, &second] {
                assert!(view.is_activatable, "{:?}", view.preparse_errors);
                assert_eq!(view.plugin_runtime.rate_limit_local_outcome_slots(), 1);
                // The real runner establishes the frame. A local pass must not
                // reach Redis, and a producer cached with a bypassed consumer
                // must still publish the outcome read by a later allowed view.
                let (mut client, server) = tokio::io::duplex(4096);
                let stream: pingora_core::protocols::Stream = Box::new(server);
                let mut session = Session::new_h1(stream);
                client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
                assert!(session.read_request().await.unwrap());
                let mut ctx = EdgionHttpContext::new();
                view.plugin_runtime.run_request_plugins(&mut session, &mut ctx).await;
                assert!(!ctx.plugin_terminated());
                assert!(session.response_written().is_none());
            }
            let invalid = generation.compile(Some(&producer_bypass));
            assert!(!invalid.is_activatable, "an active consumer requires its producer");
            assert!(!invalid.retain_previous_on_invalid_update);
        }
    }

    #[tokio::test]
    async fn rate_limit_local_dependency_checks_original_aliases_after_policy_masking() {
        use pingora_proxy::Session;
        use tokio::io::AsyncWriteExt;

        for duplicate_first in [false, true] {
            let producer = json!({"type": "RateLimitLocal", "alias": "local", "config": {
                "rate": 100, "interval": "1h", "key": [],
                "onLimitExceeded": "Continue", "showLimitHeaders": false
            }});
            let duplicate = json!({"type": "RequestHeaderModifier", "alias": "local", "config": {}});
            let mut entries = if duplicate_first {
                vec![duplicate, producer]
            } else {
                vec![producer, duplicate]
            };
            let mut ordinary = PluginGeneration::new(resource(json!({"requestPlugins": entries})));
            assert!(
                ordinary.compile(None).is_activatable,
                "ordinary duplicate identity remains nonfatal"
            );
            entries.push(json!({"type": "RateLimitRedis", "conditions": {"run": {"allOf": [
                {"type": "rateLimitExceeded", "pluginAlias": "local"}
            ]}}, "config": {
                "redisRef": "apps/intentionally-absent", "key": [],
                "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],
                "onRedisFailure": "Deny", "showLimitHeaders": false
            }}));
            let mut generation = PluginGeneration::new(resource(json!({"requestPlugins": entries})));
            let invalid = generation.compile(None);
            assert!(!invalid.is_activatable);
            assert!(!invalid.retain_previous_on_invalid_update);
            assert!(invalid
                .preparse_errors
                .iter()
                .any(|error| error.contains("unique active alias")));

            let bypass = restriction(&["RequestHeaderModifier"], PolicyActionKind::Bypass);
            let valid = generation.compile(Some(&bypass));
            assert!(valid.is_activatable, "{:?}", valid.preparse_errors);
            let (mut client, server) = tokio::io::duplex(4096);
            let stream: pingora_core::protocols::Stream = Box::new(server);
            let mut session = Session::new_h1(stream);
            client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
            assert!(session.read_request().await.unwrap());
            let mut ctx = EdgionHttpContext::new();
            valid.plugin_runtime.run_request_plugins(&mut session, &mut ctx).await;
            assert!(!ctx.plugin_terminated());
            assert!(session.response_written().is_none());
        }
    }

    #[tokio::test]
    async fn response_companions_share_and_release_with_their_request_entry() {
        for (name, config) in [
            ("ExtProc", json!({"grpcService": {"target": "processor.example:9000"}})),
            (
                "Guardrail",
                json!({
                    "target": {"url": "https://guard.example"},
                    "timeoutMs": 1000,
                    "maxResponseBytes": 4096,
                    "request": {"path": {"template": "/v1/decision"}},
                    "windowBytes": 16,
                    "maxConcurrent": 2,
                }),
            ),
        ] {
            let mut generation = PluginGeneration::new(resource(json!({
                "requestPlugins": [{"type": name, "config": config}],
            })));
            let bypass = restriction(&[name], PolicyActionKind::Bypass);
            let denied = generation.compile(Some(&bypass));
            assert_eq!(generation.cached_entry_count(), 0);
            assert_eq!(denied.plugin_runtime.upstream_response_async_plugins_iter().count(), 0);
            let first = generation.compile(None);
            let second = generation.compile(None);
            assert!(first.is_activatable, "{name}: {:?}", first.preparse_errors);
            assert!(Arc::ptr_eq(
                &request_filter(&first, name),
                &request_filter(&second, name)
            ));
            let companion = first
                .plugin_runtime
                .upstream_response_async_plugins_iter()
                .next()
                .unwrap();
            assert!(Arc::ptr_eq(
                companion,
                second
                    .plugin_runtime
                    .upstream_response_async_plugins_iter()
                    .next()
                    .unwrap()
            ));
            let weak = Arc::downgrade(companion);
            generation.release_unneeded_entries(&generation.demand(Some(&bypass)));
            drop(first);
            drop(second);
            assert!(weak.upgrade().is_none(), "{name} companion outlived all allowed views");
        }
    }

    #[test]
    fn waf_owner_and_all_stage_filters_are_shared_within_one_source() {
        let mut generation = PluginGeneration::new(resource(json!({
            "waf": {"rules": "SecAction \"id:1,phase:1,pass\"\n"},
        })));
        let first = generation.compile(None);
        let second = generation.compile(None);
        assert!(first.is_activatable, "{:?}", first.preparse_errors);
        assert_eq!(generation.cached_entry_count(), 1);
        let owner = first.plugin_runtime.waf_candidate().unwrap().owner();
        let weak = Arc::downgrade(&owner);
        assert!(Arc::ptr_eq(
            &owner,
            &second.plugin_runtime.waf_candidate().unwrap().owner()
        ));
        assert!(Arc::ptr_eq(
            first.plugin_runtime.request_plugins_iter().next().unwrap(),
            second.plugin_runtime.request_plugins_iter().next().unwrap()
        ));
        assert!(Arc::ptr_eq(
            first.plugin_runtime.upstream_response_plugins_iter().next().unwrap(),
            second.plugin_runtime.upstream_response_plugins_iter().next().unwrap()
        ));
        assert!(Arc::ptr_eq(
            first
                .plugin_runtime
                .upstream_response_async_plugins_iter()
                .next()
                .unwrap(),
            second
                .plugin_runtime
                .upstream_response_async_plugins_iter()
                .next()
                .unwrap()
        ));
        generation.release_unneeded_entries(&EntryDemand::new());
        drop(owner);
        drop(first);
        drop(second);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn failed_slot_stays_failed_and_does_not_contaminate_a_bypassed_view() {
        let mut generation = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "Dsl", "config": {"name": "invalid-program", "script": "let ="}},
            {"type": "RequestHeaderModifier", "config": {}},
        ]})));
        let first = generation.compile(None);
        let second = generation.compile(None);
        assert!(!first.is_activatable);
        assert!(!second.is_activatable);
        assert!(first.retain_previous_on_invalid_update);
        assert_eq!(first.preparse_errors, second.preparse_errors);
        assert!(matches!(
            generation.cache.get(EntrySlot {
                stage: EntryStage::Request,
                index: 0
            }),
            Some(EntryOutcome::Failed(_))
        ));
        let bypass = restriction(&["Dsl"], PolicyActionKind::Bypass);
        let allowed = generation.compile(Some(&bypass));
        assert!(allowed.is_activatable, "{:?}", allowed.preparse_errors);
        assert!(allowed.preparse_errors.is_empty());
        assert!(Arc::ptr_eq(
            &request_filter(&first, "RequestHeaderModifier"),
            &request_filter(&allowed, "RequestHeaderModifier")
        ));
        generation.release_unneeded_entries(&generation.demand(Some(&bypass)));
        assert_eq!(
            generation.cached_entry_keys().collect::<Vec<_>>(),
            vec![EntrySlot {
                stage: EntryStage::Request,
                index: 1,
            }]
        );
    }

    #[test]
    fn aliases_and_original_slots_do_not_change_when_an_earlier_entry_is_bypassed() {
        let mut generation = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "RequestHeaderModifier", "alias": "shared", "config": {}},
            {"type": "RateLimitLocal", "alias": "shared", "config": {"rate": 100}},
        ]})));
        assert!(generation.source().spec.request_plugins.as_ref().unwrap()[1]
            .alias
            .is_none());
        let bypass = restriction(&["RequestHeaderModifier"], PolicyActionKind::Bypass);
        let limited = generation.compile(Some(&bypass));
        let full = generation.compile(None);
        assert!(limited.is_activatable, "{:?}", limited.preparse_errors);
        assert_eq!(limited.preparse_errors.len(), 1);
        assert!(limited.preparse_errors[0].contains("requestPlugins[1]"));
        let limited_filter = request_filter(&limited, "RateLimitLocal");
        assert!(limited_filter.alias().is_none());
        assert!(Arc::ptr_eq(&limited_filter, &request_filter(&full, "RateLimitLocal")));
        generation.release_unneeded_entries(&generation.demand(Some(&bypass)));
        assert_eq!(
            generation.cached_entry_keys().collect::<Vec<_>>(),
            vec![EntrySlot {
                stage: EntryStage::Request,
                index: 1,
            }]
        );
    }

    #[test]
    fn metadata_and_cross_entry_validation_are_rebuilt_for_each_scope() {
        let mut generation = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "Cors", "config": {"allowOrigins": "https://example.com"}},
            {"type": "RequestId", "config": {}},
            {"type": "AiProxy", "config": {}},
            {"type": "AiGuard", "config": {"rules": [{
                "name": "guard-rule",
                "match": {"type": "regex", "pattern": "private-prompt"},
                "action": "reject",
            }]}},
        ]})));
        let full = generation.compile(None);
        assert!(!full.is_activatable, "AiGuard after AiProxy must be rejected");
        assert!(!full.retain_previous_on_invalid_update);
        assert!(full.plugin_runtime.cors_config_arc().is_some());
        assert!(full.plugin_runtime.has_request_id());
        let bypass = restriction(&["Cors", "RequestId", "AiProxy"], PolicyActionKind::Bypass);
        let reduced = generation.compile(Some(&bypass));
        assert!(reduced.is_activatable, "{:?}", reduced.preparse_errors);
        assert!(reduced.plugin_runtime.cors_config_arc().is_none());
        assert!(!reduced.plugin_runtime.has_request_id());
        let full_again = generation.compile(None);
        assert!(!full_again.is_activatable);
        assert!(Arc::ptr_eq(
            &full.plugin_runtime.cors_config_arc().unwrap(),
            &full_again.plugin_runtime.cors_config_arc().unwrap()
        ));
    }

    #[tokio::test]
    async fn shared_body_wrapper_resolves_the_current_request_scope_cap() {
        let mut generation = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "RequestBodyBuffer", "config": {}},
        ]})));
        let first = generation.compile(None);
        let second = generation.compile(None);
        let filter = request_filter(&first, "RequestBodyBuffer");
        assert!(Arc::ptr_eq(&filter, &request_filter(&second, "RequestBodyBuffer")));
        for cap in [1024, 8192] {
            let mut ctx = EdgionHttpContext::new();
            ctx.request_body_config.max_body_size_bytes = cap;
            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx);
            session
                .expect_ensure_request_body()
                .times(1)
                .withf(move |limit| *limit == Some(cap))
                .returning(|_| Ok(0));
            let mut log = PluginLog::new("RequestBodyBuffer");
            assert_eq!(
                filter.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            );
        }
    }

    #[tokio::test]
    async fn latest_source_block_precedes_restricted_retained_candidate_entries() {
        let latest = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "RequestHeaderModifier", "config": {}},
        ]})));
        let mut candidate = PluginGeneration::new(resource(json!({"requestPlugins": [
            {"type": "RateLimitLocal", "config": {"rate": 100}},
        ]})));
        let policy = PluginPolicyConfig {
            deny: vec![
                PluginPolicyEntryConfig {
                    name: "http/RequestHeaderModifier".into(),
                    action: Some(PolicyActionKind::Block),
                    block_status: Some(451),
                },
                PluginPolicyEntryConfig {
                    name: "http/RateLimitLocal".into(),
                    action: Some(PolicyActionKind::Block),
                    block_status: Some(403),
                },
            ],
            ..Default::default()
        }
        .compile()
        .unwrap();
        let actions = latest.policy_actions(Some(&policy));
        let retained = candidate.compile_with_additional_actions(Some(&policy), &actions);
        assert_eq!(candidate.cached_entry_count(), 0);
        let gate = retained.plugin_runtime.request_plugins_iter().next().unwrap();
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("PluginPolicyBlock");
        assert!(matches!(
            gate.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 451,
                body: Some(_)
            }
        ));
    }
}
