use super::*;
use bytes::Bytes;
use serde_json::json;

fn owner(spec: serde_json::Value) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"transport","namespace":"apps"},"spec":spec})).unwrap()
}

fn managed_runtime_owner(kind: &str) -> edgion_resources::EdgionPlugins {
    let connection = json!({"target":{"name":"guard","port":8080},"request":{"path":{"template":"/decision"}}});
    let config = if kind == "AiGuard" {
        json!({"evaluator":connection})
    } else {
        connection
    };
    owner(json!({"requestPlugins":[{"type":kind,"config":config}]}))
}

fn managed_template_owner(kind: &str, material: serde_json::Value) -> edgion_resources::EdgionPlugins {
    let mut wire = serde_json::to_value(managed_runtime_owner(kind)).unwrap();
    let config = &mut wire["spec"]["requestPlugins"][0]["config"];
    let config = if kind == "AiGuard" {
        &mut config["evaluator"]
    } else {
        config
    };
    config["request"] =
        json!({"path":{"template":"/decision/${secretRef:apps/token:key}/${secretRef:apps/second:key}"}});
    config["resolvedSecrets"] = material;
    serde_json::from_value(wire).unwrap()
}

#[test]
fn ai_membership_follows_successful_construction_including_guardrail_companion() {
    let ai_proxy = owner(json!({"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat"}}]}));
    let ai_guard = owner(json!({"requestPlugins":[{"type":"AiGuard","config":{"rules":[{
        "name":"tripwire","match":{"type":"regex","pattern":"private"},"action":"reject"
    }]}}]}));
    for mut source in [
        ai_proxy,
        ai_guard,
        managed_runtime_owner("Guardrail"),
        managed_runtime_owner("AiGuard"),
    ] {
        source.spec.request_plugins.as_mut().unwrap()[0].conditions = serde_json::from_value(json!({
            "run":{"allOf":[{"type":"keyExist","key":{"type":"header","name":"x-run"}}]}
        }))
        .unwrap();
        let mut cache = EntryCache::default();
        for _ in 0..2 {
            let mut runtime = PluginRuntime::new();
            let errors = runtime.add_resource_filters_with_cache(&source, &mut cache);
            assert!(errors.is_empty(), "{errors:?}");
            assert!(runtime.has_ai_conversion_owner());
            assert!(!runtime.has_unary_body_consumer());
            assert_eq!(runtime.request_plugins_count(), 1);
        }
        source.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        let mut disabled = PluginRuntime::new();
        assert!(disabled.add_resource_filters_with_cache(&source, &mut cache).is_empty());
        assert!(!disabled.has_ai_conversion_owner());
    }
}

#[test]
fn invalid_ai_owner_cannot_publish_or_replay_cached_membership() {
    for kind in ["Guardrail", "AiGuard"] {
        let valid = managed_template_owner(kind, json!({"apps/token:key":"one","apps/second:key":"two"}));
        let mut cache = EntryCache::default();
        let mut first = PluginRuntime::new();
        assert!(first.add_resource_filters_with_cache(&valid, &mut cache).is_empty());
        assert!(first.has_ai_conversion_owner());
        let missing = managed_template_owner(kind, json!({}));
        let mut rejected = PluginRuntime::new();
        assert!(!rejected
            .add_resource_filters_with_cache(&missing, &mut cache)
            .is_empty());
        assert!(rejected.has_invalid_plugins());
        assert!(!rejected.has_ai_conversion_owner());
        assert_eq!(rejected.request_plugins_count(), 0);
        assert_eq!(rejected.upstream_response_async_plugins_count(), 0);
        assert!(
            first.has_ai_conversion_owner(),
            "captured executable view remains immutable"
        );
    }
    let invalid_proxy = owner(
        json!({"requestPlugins":[{"type":"AiProxy","config":{"api":"anthropicMessages","conversion":"chatToAnthropic"}}]}),
    );
    let compiled = crate::plugins::http::compile_plugins(&invalid_proxy);
    assert!(!compiled.is_activatable);
    assert!(!compiled.plugin_runtime.has_ai_conversion_owner());
}

#[test]
#[cfg(feature = "wasm")]
fn unary_membership_is_stage_specific_and_requires_executable_body_filter() {
    let wasm = json!({"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/header_demo.wasm")},"pluginConfig":{"string":"transport-probe"}}});
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
    ] {
        let mut source = owner(json!({stage:[wasm.clone()]}));
        let active = crate::plugins::http::compile_plugins(&source);
        assert!(active.is_activatable, "{stage}: {:?}", active.preparse_errors);
        assert_eq!(
            active.plugin_runtime.has_unary_body_consumer(),
            stage == "upstreamResponseBodyFilterPlugins"
        );
        assert!(!active.plugin_runtime.has_ai_conversion_owner());
        if let Some(entries) = source.spec.upstream_response_body_filter_plugins.as_mut() {
            entries[0].enable = false;
        }
        if stage == "upstreamResponseBodyFilterPlugins" {
            let disabled = crate::plugins::http::compile_plugins(&source);
            assert!(!disabled.plugin_runtime.has_unary_body_consumer());
        }
    }
    // A missing local module still constructs an executable runtime: module
    // loading is best-effort preload plus request-time failure policy.
    let lazy = crate::plugins::http::compile_plugins(&owner(json!({
        "upstreamResponseBodyFilterPlugins":[{"type":"Wasm","config":{"source":{"file":"missing-transport-fixture.wasm"}}}]
    })));
    assert!(lazy.is_activatable);
    assert!(lazy.plugin_runtime.has_unary_body_consumer());
    for spec in [
        json!({"upstreamResponseBodyFilterPlugins":[{"type":"Wasm","config":{"source":{}}}]}),
        json!({"upstreamResponseBodyFilterPlugins":[{"type":"AiProxy","config":{"api":"openaiChat"}}]}),
        json!({"requestPlugins":[{"type":"BandwidthLimit","config":{"rate":"100kb"}}]}),
    ] {
        let rejected = crate::plugins::http::compile_plugins(&owner(spec));
        assert!(!rejected.is_activatable);
        assert!(!rejected.plugin_runtime.has_unary_body_consumer());
        assert!(!rejected.plugin_runtime.has_ai_conversion_owner());
    }
}

#[test]
fn entry_outcome_label_does_not_hide_successful_executable_members() {
    let mut runtime = PluginRuntime::new();
    let source = owner(json!({"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat"}}]}));
    assert!(runtime
        .add_resource_filters_with_cache(&source, &mut EntryCache::default())
        .is_empty());
    runtime.invalid_plugins.record_non_retainable("InvalidSibling");
    let outcome = runtime.into_entry_outcome(vec!["invalid sibling".into()]);
    assert!(matches!(outcome, EntryOutcome::Failed(_)));
    let mut view = PluginRuntime::new();
    view.append_entry_bundle(outcome.bundle());
    assert!(view.has_ai_conversion_owner());
    assert_eq!(view.request_plugins_count(), 1);
    assert!(view.has_invalid_plugins());
    let mut empty = PluginRuntime::new();
    empty.invalid_plugins.record_non_retainable("AiProxy");
    let failed = empty.into_entry_outcome(Vec::new());
    let mut view = PluginRuntime::new();
    view.append_entry_bundle(failed.bundle());
    assert!(!view.has_ai_conversion_owner());
}

struct LocalReplyTransportFilter(u8);

#[async_trait::async_trait]
impl RequestFilter for LocalReplyTransportFilter {
    fn name(&self) -> &str {
        "LocalReplyTransportTest"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
        match self.0 {
            0 => PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(b"denied")),
            },
            1 => {
                let mut header = ResponseHeader::build(401, Some(2)).unwrap();
                header.insert_header("content-length", "6").unwrap();
                session.write_response_header(Box::new(header), false).await.unwrap();
                session
                    .write_response_body(Some(Bytes::from_static(b"denied")), true)
                    .await
                    .unwrap();
                PluginRunningResult::ErrTerminateRequest
            }
            2 => PluginRunningResult::ErrTerminateRequest,
            _ => PluginRunningResult::GoodNext,
        }
    }
}

#[tokio::test]
async fn real_backend_dispatch_closes_h1_for_reply_and_bare_termination_only() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (backend, mode) in [(true, 0), (true, 1), (true, 2), (true, 3), (false, 0)] {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (mut client, server) = tokio::io::duplex(64 * 1024);
            client.write_all(b"GET /reply HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
            let mut session = Session::new_h1(Box::new(server));
            session.read_request().await.unwrap();
            let mut ctx = EdgionHttpContext::default();
            let mut runtime = PluginRuntime::new();
            runtime.request_plugins.push(Arc::new(LocalReplyTransportFilter(mode)));
            if backend { runtime.run_backend_request_plugins(&mut session, &mut ctx).await; }
            else { runtime.run_request_plugins(&mut session, &mut ctx).await; }
            if mode == 3 {
                assert!(!ctx.plugin_terminated());
                assert!(session.get_keepalive().is_some());
                return;
            }
            assert!(ctx.plugin_terminated());
            if mode == 2 {
                assert!(!ctx.response_write_started());
                assert!(session.get_keepalive().is_none(), "dispatch closes before the existing fallback writer");
                crate::terminate_with_fallback(&mut session, &mut ctx, &crate::runtime::server::ServerHeaderOpts::default()).await.unwrap();
            }
            assert_eq!(session.get_keepalive().is_none(), backend);
            let reusable = session.downstream_session.finish().await.unwrap();
            assert_eq!(reusable.is_none(), backend);
            drop(reusable);
            let mut wire = Vec::new();
            client.read_to_end(&mut wire).await.unwrap();
            let text = String::from_utf8(wire).unwrap();
            let (head, body) = text.split_once("\r\n\r\n").unwrap();
            assert!(head.starts_with(if mode == 2 { "HTTP/1.1 502" } else { "HTTP/1.1 401" }));
            assert_eq!(head.to_ascii_lowercase().contains("connection: close"), backend);
            if mode == 2 {
                assert_eq!(body.as_bytes(), b"<html>\n<head><title>502 Bad Gateway</title></head>\n<body>\n<center><h1>502 Bad Gateway</h1></center>\n<hr><center>edgion</center>\n</body>\n</html>");
            } else { assert_eq!(body.as_bytes(), b"denied"); }
        }).await.unwrap();
    }
}
