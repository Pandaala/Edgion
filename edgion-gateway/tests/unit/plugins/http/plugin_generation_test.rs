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
        ]}}, "config": {"quotas":[{"name":"default", "key": [],
            "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],}],
            "redisRef": "apps/intentionally-absent",
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
        ]}}, "config": {"quotas":[{"name":"default", "key": [],
            "policies": [{"rate": 1, "interval": "1h", "algorithm": "TokenBucket"}],}],
            "redisRef": "apps/intentionally-absent",
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
    let mut source = resource(json!({}));
    source.spec.waf = Some(crate::plugins::http::waf::fixture::config_in_namespace(
        "SecAction \"id:1,phase:1,pass\"\n",
        "apps",
    ));
    let mut generation = PluginGeneration::new(source);
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
        {"type": "AiProxy", "config": {"api":"openaiChat","usageObservation":"provider",}},
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

#[tokio::test]
async fn unsupported_types_fail_closed_even_when_disabled_or_policy_masked() {
    let bypass = PluginPolicyConfig {
        allow: Some(Vec::new()),
        denied_action: PolicyActionKind::Bypass,
        ..Default::default()
    }
    .compile()
    .unwrap();
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        for enabled in [true, false] {
            for policy in [None, Some(&bypass)] {
                let source = resource(json!({stage: [{"type":"FuturePlugin", "enable":enabled, "config":{}}]}));
                let expected = source.validate_plugins();
                let mut generation = PluginGeneration::new(source);
                let view = generation.compile(policy);
                assert!(!view.is_activatable);
                assert!(!view.retain_previous_on_invalid_update);
                assert_eq!(view.preparse_errors, expected);
                let gate = view.plugin_runtime.request_plugins_iter().next().unwrap();
                let mut session = MockPluginSession::new();
                let mut log = PluginLog::new("UnsupportedTest");
                assert!(matches!(
                    gate.run_request(&mut session, &mut log).await,
                    PluginRunningResult::ErrResponse { status: 500, .. }
                ));
            }
        }
    }
    let corrected = resource(json!({"requestPlugins":[{"type":"RequestHeaderModifier", "config":{}}]}));
    assert!(PluginGeneration::new(corrected).compile(None).is_activatable);
}

#[test]
fn mirror_generation_policy_masks_keep_owner_ordinals_and_generation_cache() {
    let mut source = resource(json!({"requestPlugins":[
        {"type":"RequestMirror","enable":false,"config":{"backendRef":{"name":"disabled","port":80}}},
        {"type":"RequestMirror","config":{"backendRef":{"name":"active","port":80}}}
    ]}));
    source.metadata.uid = Some("generation-a".into());
    source.metadata.generation = Some(3);
    let mut generation = PluginGeneration::new(source.clone());
    let active = generation.compile(None);
    assert!(active.is_activatable);
    assert_eq!(active.spec.spec.request_plugins.as_ref().unwrap().len(), 2);
    assert!(generation
        .cache
        .get(EntrySlot {
            stage: EntryStage::Request,
            index: 0
        })
        .is_none());
    assert!(generation
        .cache
        .get(EntrySlot {
            stage: EntryStage::Request,
            index: 1
        })
        .is_some());
    let original = request_filter(&active, "RequestMirror");
    let masked = generation.compile(Some(&restriction(&["RequestMirror"], PolicyActionKind::Bypass)));
    assert_eq!(masked.spec.spec.request_plugins.as_ref().unwrap().len(), 2);
    assert!(masked
        .plugin_runtime
        .request_plugins_iter()
        .all(|filter| filter.name() != "RequestMirror"));
    assert_eq!(masked.spec.metadata.uid.as_deref(), Some("generation-a"));
    let reopened = generation.compile(None);
    assert!(Arc::ptr_eq(&original, &request_filter(&reopened, "RequestMirror")));
    source.metadata.uid = Some("generation-b".into());
    source.metadata.generation = Some(1);
    let newer = PluginGeneration::new(source).compile(None);
    assert!(!Arc::ptr_eq(&original, &request_filter(&newer, "RequestMirror")));
}

#[test]
fn descriptor_generations_preserve_masked_ordinals_and_old_runtime_lifetimes() {
    let descriptor = include_bytes!("../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
    let mut source = resource(json!({"requestPlugins":[
        {"type":"GrpcJsonTranscoder","enable":false,"config":{
            "descriptor":{"type":"configMapRef","name":"bad/name","key":""},"services":["test.TestService"],"resolvedDescriptor":descriptor.to_vec()}},
        {"type":"GrpcJsonTranscoder","config":{
            "descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"],"resolvedDescriptor":descriptor.to_vec()}}
    ]}));
    source.metadata.uid = Some("first".into());
    source.metadata.generation = Some(1);
    let mut generation = PluginGeneration::new(source.clone());
    let bypass = restriction(&["GrpcJsonTranscoder"], PolicyActionKind::Bypass);
    let masked = generation.compile(Some(&bypass));
    assert!(masked.is_activatable);
    assert_eq!(generation.cached_entry_count(), 0);
    let active = generation.compile(None);
    assert!(active.is_activatable, "{:?}", active.preparse_errors);
    let original = request_filter(&active, "GrpcJsonTranscoder");
    let keys: Vec<_> = generation.cached_entry_keys().collect();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].index, 1);
    let again = generation.compile(None);
    assert!(Arc::ptr_eq(&original, &request_filter(&again, "GrpcJsonTranscoder")));
    let masked_again = generation.compile(Some(&bypass));
    assert!(!masked_again
        .plugin_runtime
        .request_plugins_iter()
        .any(|filter| filter.name() == "GrpcJsonTranscoder"));
    let mut changed = source.clone();
    changed.metadata.uid = Some("recreated".into());
    changed.metadata.generation = Some(2);
    let mut replacement = PluginGeneration::new(changed.clone());
    let current = replacement.compile(None);
    assert!(current.is_activatable);
    assert!(!Arc::ptr_eq(&original, &request_filter(&current, "GrpcJsonTranscoder")));
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::GrpcJsonTranscoder(config) =
        &mut changed.spec.request_plugins.as_mut().unwrap()[1].plugin
    else {
        panic!()
    };
    config.resolved_descriptor = None;
    let mut missing = PluginGeneration::new(changed.clone());
    assert!(!missing.compile(None).is_activatable);
    changed.spec.request_plugins.as_mut().unwrap()[1].enable = false;
    assert!(PluginGeneration::new(changed).compile(None).is_activatable);
    drop(generation);
    drop(replacement);
    assert_eq!(original.name(), "GrpcJsonTranscoder");
    assert!(Arc::ptr_eq(&original, &request_filter(&active, "GrpcJsonTranscoder")));
}

#[test]
fn descriptor_policy_view_omits_material_before_copy_and_preserves_generation_source() {
    let descriptor = include_bytes!("../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
    let source = resource(json!({"requestPlugins":[
        {"type":"RequestHeaderModifier","config":{"set":[{"name":"X-Sibling","value":"kept"}]}},
        {"type":"GrpcJsonTranscoder","config":{"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"],"resolvedDescriptor":descriptor.to_vec()}}
    ]}));
    let mut generation = PluginGeneration::from_source(&source);
    let active = generation.compile(None);
    assert!(active.is_activatable);
    let previous = request_filter(&active, "GrpcJsonTranscoder");
    let masked = generation.compile(Some(&restriction(&["GrpcJsonTranscoder"], PolicyActionKind::Bypass)));
    assert!(masked.is_activatable);
    assert_eq!(masked.spec.spec.request_plugins.as_ref().unwrap().len(), 2);
    let edgion_resources::EdgionPlugin::GrpcJsonTranscoder(masked_config) =
        &masked.spec.spec.request_plugins.as_ref().unwrap()[1].plugin
    else {
        panic!()
    };
    assert!(masked_config.resolved_descriptor.is_none());
    let edgion_resources::EdgionPlugin::GrpcJsonTranscoder(source_config) =
        &generation.source().spec.request_plugins.as_ref().unwrap()[1].plugin
    else {
        panic!()
    };
    assert_eq!(
        source_config.resolved_descriptor.as_deref(),
        Some(descriptor.as_slice())
    );
    assert!(Arc::ptr_eq(
        &request_filter(&active, "RequestHeaderModifier"),
        &request_filter(&masked, "RequestHeaderModifier")
    ));
    let resumed = generation.compile(None);
    assert!(Arc::ptr_eq(&previous, &request_filter(&resumed, "GrpcJsonTranscoder")));
}

#[tokio::test]
async fn owning_ip_generation_defaults_namespace_keeps_pinned_target_and_live_content() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    let seed = |name: &str, cidr: &str| {
        let data: edgion_resources::EdgionConfigData = serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"nets","cidrs":[cidr]}]}}}})).unwrap();
        get_global_edgion_config_data_store().insert_raw_for_test(format!("default/{name}"), Arc::new(data));
    };
    let mut owner = resource(
        json!({"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"r5c2-http-old"}]}}]}),
    );
    owner.metadata.namespace = None;
    let mut generation = PluginGeneration::new(owner.clone());
    let first = generation.compile(None);
    let filter = request_filter(&first, "IpRestriction");
    let mut missing = MockPluginSession::new();
    missing.expect_remote_ip().return_const("198.51.100.8".to_owned());
    missing.expect_write_response_header().returning(|_, _| Ok(()));
    missing.expect_write_response_body().returning(|_, _| Ok(()));
    missing.expect_shutdown().returning(|| {});
    assert_eq!(
        filter
            .run_request(&mut missing, &mut PluginLog::new("IpRestriction"))
            .await,
        PluginRunningResult::ErrTerminateRequest
    );
    seed("r5c2-http-old", "198.51.100.8/32");
    assert!(Arc::ptr_eq(
        &filter,
        &request_filter(&generation.compile(None), "IpRestriction")
    ));
    let bypass = generation.compile(Some(&restriction(&["IpRestriction"], PolicyActionKind::Bypass)));
    assert!(bypass
        .plugin_runtime
        .request_plugins_iter()
        .all(|f| f.name() != "IpRestriction"));
    let mut next = owner;
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::IpRestriction(config) =
        &mut next.spec.request_plugins.as_mut().unwrap()[0].plugin
    else {
        panic!()
    };
    config.allow_refs.as_mut().unwrap()[0].name = "r5c2-http-new".into();
    seed("r5c2-http-new", "203.0.113.8/32");
    let next_view = PluginGeneration::new(next).compile(None);
    let next_filter = request_filter(&next_view, "IpRestriction");
    for (filter, allow) in [(&filter, true), (&next_filter, false)] {
        let mut session = MockPluginSession::new();
        session.expect_remote_ip().return_const("198.51.100.8".to_owned());
        if !allow {
            session.expect_write_response_header().returning(|_, _| Ok(()));
            session.expect_write_response_body().returning(|_, _| Ok(()));
            session.expect_shutdown().returning(|| {});
        }
        assert_eq!(
            filter
                .run_request(&mut session, &mut PluginLog::new("IpRestriction"))
                .await
                == PluginRunningResult::GoodNext,
            allow
        );
    }
    seed("r5c2-http-old", "203.0.113.8/32");
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const("198.51.100.8".to_owned());
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    assert_eq!(
        filter
            .run_request(&mut session, &mut PluginLog::new("IpRestriction"))
            .await,
        PluginRunningResult::ErrTerminateRequest
    );
}

#[test]
fn transport_facts_follow_policy_views_cache_release_and_retained_generations() {
    for (stage, entry, ai, unary) in [
        (
            "requestPlugins",
            json!({"type":"AiProxy","config":{"api":"openaiChat"}}),
            true,
            false,
        ),
        (
            "requestPlugins",
            json!({"type":"AiGuard","config":{"rules":[{"name":"tripwire","match":{"type":"regex","pattern":"private"},"action":"reject"}]}}),
            true,
            false,
        ),
        (
            "requestPlugins",
            json!({"type":"Guardrail","config":{"target":{"name":"guard","port":8080},"request":{"path":{"template":"/decision"}}}}),
            true,
            false,
        ),
        (
            "upstreamResponseBodyFilterPlugins",
            json!({"type":"BandwidthLimit","config":{"rate":"100kb"}}),
            false,
            true,
        ),
        (
            "upstreamResponseBodyFilterPlugins",
            json!({"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/header_demo.wasm")},"pluginConfig":{"string":"generation-transport-probe"}}}),
            false,
            true,
        ),
    ] {
        let name = entry["type"].as_str().unwrap();
        for action in [PolicyActionKind::Block, PolicyActionKind::Bypass] {
            let policy = restriction(&[name], action);
            let source = resource(json!({stage:[entry.clone()]}));
            let mut generation = PluginGeneration::new(source.clone());
            let masked_first = generation.compile(Some(&policy));
            assert!(masked_first.is_activatable, "{:?}", masked_first.preparse_errors);
            assert_eq!(generation.cached_entry_count(), 0);
            assert!(!masked_first.plugin_runtime.has_ai_conversion_owner());
            assert!(!masked_first.plugin_runtime.has_unary_body_consumer());
            let active = generation.compile(None);
            assert!(active.is_activatable, "{name}: {:?}", active.preparse_errors);
            assert_eq!(active.plugin_runtime.has_ai_conversion_owner(), ai);
            assert_eq!(active.plugin_runtime.has_unary_body_consumer(), unary);
            let masked_again = generation.compile(Some(&policy));
            assert!(!masked_again.plugin_runtime.has_ai_conversion_owner());
            assert!(!masked_again.plugin_runtime.has_unary_body_consumer());
            let reopened = generation.compile(None);
            assert_eq!(reopened.plugin_runtime.has_ai_conversion_owner(), ai);
            assert_eq!(reopened.plugin_runtime.has_unary_body_consumer(), unary);
            if ai {
                let filter = request_filter(&active, name);
                assert!(Arc::ptr_eq(&filter, &request_filter(&reopened, name)));
                let weak = Arc::downgrade(&filter);
                generation.release_unneeded_entries(&generation.demand(Some(&policy)));
                assert_eq!(generation.cached_entry_count(), 0);
                drop(filter);
                drop(reopened);
                assert!(weak.upgrade().is_some());
                assert!(active.plugin_runtime.has_ai_conversion_owner());
                drop(active);
                assert!(weak.upgrade().is_none());
            } else {
                let filter = active
                    .plugin_runtime
                    .upstream_response_body_plugins_iter()
                    .next()
                    .unwrap()
                    .clone();
                assert!(Arc::ptr_eq(
                    &filter,
                    reopened
                        .plugin_runtime
                        .upstream_response_body_plugins_iter()
                        .next()
                        .unwrap()
                ));
                let weak = Arc::downgrade(&filter);
                generation.release_unneeded_entries(&generation.demand(Some(&policy)));
                assert_eq!(generation.cached_entry_count(), 0);
                drop(filter);
                drop(reopened);
                assert!(weak.upgrade().is_some());
                assert!(active.plugin_runtime.has_unary_body_consumer());
                drop(active);
                assert!(weak.upgrade().is_none());
            }
            let latest = PluginGeneration::new(resource(
                json!({"requestPlugins":[{"type":"RequestHeaderModifier","config":{}}]}),
            ));
            let latest_actions =
                latest.policy_actions(Some(&restriction(&["RequestHeaderModifier"], PolicyActionKind::Block)));
            let retained = generation.compile_with_additional_actions(Some(&policy), &latest_actions);
            assert!(!retained.plugin_runtime.has_ai_conversion_owner());
            assert!(!retained.plugin_runtime.has_unary_body_consumer());
            let old_allowed = generation.compile(None);
            let replacement = PluginGeneration::new(resource(json!({}))).compile(None);
            assert_eq!(old_allowed.plugin_runtime.has_ai_conversion_owner(), ai);
            assert_eq!(old_allowed.plugin_runtime.has_unary_body_consumer(), unary);
            assert!(!replacement.plugin_runtime.has_ai_conversion_owner());
            assert!(!replacement.plugin_runtime.has_unary_body_consumer());
        }
    }
}

#[test]
fn failed_transport_entries_never_publish_cached_raw_category() {
    for (stage, entry) in [
        (
            "requestPlugins",
            json!({"type":"AiProxy","config":{"api":"anthropicMessages","conversion":"chatToAnthropic"}}),
        ),
        (
            "upstreamResponseBodyFilterPlugins",
            json!({"type":"Wasm","config":{"source":{}}}),
        ),
    ] {
        let name = entry["type"].as_str().unwrap();
        let mut generation = PluginGeneration::new(resource(json!({stage:[entry.clone()]})));
        for _ in 0..2 {
            let failed = generation.compile(None);
            assert!(!failed.is_activatable);
            assert!(!failed.plugin_runtime.has_ai_conversion_owner());
            assert!(!failed.plugin_runtime.has_unary_body_consumer());
        }
        let masked = generation.compile(Some(&restriction(&[name], PolicyActionKind::Bypass)));
        assert!(masked.is_activatable);
        assert!(!masked.plugin_runtime.has_ai_conversion_owner());
        assert!(!masked.plugin_runtime.has_unary_body_consumer());
    }
}
