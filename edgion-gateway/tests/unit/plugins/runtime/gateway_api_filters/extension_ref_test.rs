use super::*;
use crate::plugins::http::compile_plugins;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
use edgion_resources::resources::http_route::HTTPHeaderFilter;
use kube::core::ObjectMeta;
use mockall::predicate::eq;

#[test]
fn plugin_key_accessor_is_allocation_free() {
    let filter = ExtensionRefFilter::new(
        "production".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "api-guard".into(),
            resolved_namespace: None,
        },
    );

    let (key, allocations) =
        crate::test_allocation_counter::count_allocation_stats(|| std::hint::black_box(filter.plugin_key()));

    assert_eq!(key, "production/api-guard");
    assert_eq!(allocations.calls, 0);
}

fn make_plugin(key_ns: &str, name: &str) -> Arc<PluginRuntime> {
    let ep = EdgionPlugins {
        metadata: ObjectMeta {
            namespace: Some(key_ns.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                HTTPHeaderFilter {
                    set: None,
                    add: None,
                    remove: None,
                },
            ))]),
            ..Default::default()
        },
        status: None,
    };

    Arc::clone(&compile_plugins(&ep).plugin_runtime)
}

fn make_terminating_plugin(key_ns: &str, name: &str) -> Arc<crate::plugins::runtime::PluginRuntime> {
    let ep = EdgionPlugins {
        metadata: ObjectMeta {
            namespace: Some(key_ns.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Mock(
                edgion_resources::resources::edgion_plugins::MockConfig::default(),
            ))]),
            ..Default::default()
        },
        status: None,
    };
    Arc::clone(&compile_plugins(&ep).plugin_runtime)
}

#[tokio::test]
async fn request_inner_identity_restores_outer_on_early_result() {
    let runtime = make_terminating_plugin("ns", "terminating");
    let runtime_for_resolve = Arc::clone(&runtime);
    let outer = crate::ctx::CurrentPluginEntryIdentity::new("ExtensionRef", Some("outer"));
    let outer_for_replace = outer;

    let mut session = MockPluginSession::new();
    session
        .expect_resolved_route_ext_ref()
        .with(eq(0usize))
        .times(1)
        .returning(move |_| Some(Arc::clone(&runtime_for_resolve)));
    session
        .expect_start_edgion_plugins_log()
        .returning(|_| EdgionPluginsLogToken::new(0));
    session.expect_install_dye_plan().return_const(());
    session.expect_strip_request_dye_once().return_const(());
    session
        .expect_replace_plugin_entry_identity()
        .times(1)
        .withf(|identity| identity.as_ref().is_some_and(|value| value.name() == "Mock"))
        .returning(move |_| outer_for_replace);
    session
        .expect_restore_plugin_entry_identity()
        .times(1)
        .withf(|identity| identity.as_ref().is_some_and(|value| value.name() == "ExtensionRef"))
        .return_const(());
    session.expect_set_response_header().times(1).returning(|_, _| Ok(()));
    session.expect_push_to_edgion_plugins_log().times(1).return_const(());

    let mut filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "terminating".into(),
            resolved_namespace: None,
        },
    );
    filter.set_index(0);
    let mut log = PluginLog::new("test");
    let result = filter.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 200, .. }));
}

#[tokio::test]
async fn request_extension_suppresses_inner_waf_plugin_log() {
    let mut inner = crate::plugins::runtime::PluginRuntime::new();
    inner
        .add_waf(&edgion_resources::resources::edgion_plugins::WafConfig {
            rules: "SecAction \"id:1,phase:1,pass,log\"\n".to_string(),
            mode: edgion_resources::resources::edgion_plugins::WafMode::On,
            priority: 0,
            request_body: Default::default(),
            ..edgion_resources::resources::edgion_plugins::WafConfig::default()
        })
        .unwrap();
    let inner = Arc::new(inner);
    let inner_for_resolve = Arc::clone(&inner);

    let mut session = MockPluginSession::new();
    session
        .expect_resolved_route_ext_ref()
        .with(eq(0usize))
        .times(1)
        .returning(move |_| Some(Arc::clone(&inner_for_resolve)));
    session
        .expect_start_edgion_plugins_log()
        .returning(|_| EdgionPluginsLogToken::new(0));
    session.expect_install_dye_plan().return_const(());
    session.expect_strip_request_dye_once().return_const(());
    session
        .expect_replace_plugin_entry_identity()
        .times(1)
        .returning(|_| None);
    session.expect_restore_plugin_entry_identity().times(1).return_const(());
    session
        .expect_claim_waf_phase()
        .returning(|_, _| crate::ctx::WafPhaseClaim::Execute);
    session.expect_request_header_map().return_const(http::HeaderMap::new());
    session.expect_get_method().times(1).return_const("GET".to_string());
    session.expect_get_uri().return_const("/".to_string());
    session.expect_get_request_uri().return_const("/".to_string());
    session.expect_get_http_protocol().return_const("HTTP/1.1".to_string());
    session.expect_remote_ip().return_const("192.0.2.1".to_string());
    session.expect_get_request_id().return_const("request-1".to_string());
    session.expect_record_waf_phase_outcome().returning(|_, _, _, _| ());
    session.expect_install_waf_transaction().return_once(|_, transaction| {
        drop(transaction);
        true
    });
    session.expect_push_to_edgion_plugins_log().times(0);

    let mut filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "waf".into(),
            resolved_namespace: None,
        },
    );
    filter.set_index(0);
    let mut log = PluginLog::new("ExtensionRef");

    let result = filter.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.refer_to.as_deref(), Some("waf"));
}

#[test]
fn backend_scope_uses_request_pinned_runtime_across_attempts() {
    use crate::plugins::runtime::log::EdgionPluginsLogToken;

    // Prepare store with plugin
    let runtime = make_plugin("ns", "p1");

    let mut session = MockPluginSession::new();
    session
        .expect_pinned_backend_ext_ref()
        .with(eq("ns/p1"))
        .times(2)
        .returning(move |_| Some(Arc::clone(&runtime)));
    session
        .expect_start_edgion_plugins_log()
        .times(2)
        .returning(|_| EdgionPluginsLogToken::new(0));

    let filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "p1".into(),
            resolved_namespace: None,
        },
    );
    let mut log = PluginLog::new("test");
    for _ in 0..2 {
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
    // Verify refer_to is set
    assert!(log.refer_to.is_some());
    assert_eq!(log.refer_to.as_ref().unwrap(), "p1");
}

#[test]
fn backend_scope_rejects_upstream_jump_runtime() {
    let ep = EdgionPlugins {
        metadata: ObjectMeta {
            namespace: Some("ns".to_string()),
            name: Some("jump".to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::DirectEndpoint(
                edgion_resources::resources::edgion_plugins::DirectEndpointConfig::default(),
            ))]),
            ..Default::default()
        },
        status: None,
    };
    let runtime = Arc::clone(&compile_plugins(&ep).plugin_runtime);
    assert!(runtime.has_upstream_jump_plugin());
    let mut session = MockPluginSession::new();
    session
        .expect_pinned_backend_ext_ref()
        .with(eq("ns/jump"))
        .once()
        .returning(move |_| Some(Arc::clone(&runtime)));
    let filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "jump".into(),
            resolved_namespace: None,
        },
    );
    let mut log = PluginLog::new("ExtensionRef");
    assert!(filter.prepare_execution(&mut session, &mut log).is_err());
    assert!(log.render_for_test().contains("upstream jump plugin"));

    let mut disabled = ep;
    disabled.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    assert!(!compile_plugins(&disabled).plugin_runtime.has_upstream_jump_plugin());
}

/// Rule-scope filter with valid index: resolved_route_ext_ref returns Some.
/// prepare_execution should not touch the global store; it uses the
/// pre-resolved target. Run via the sync upstream_response_filter stage so
/// the test stays simple.
#[test]
fn index_hit_uses_resolved_target() {
    use crate::plugins::runtime::PluginRuntime;

    let target = std::sync::Arc::new(PluginRuntime::new());
    let target_for_mock = std::sync::Arc::clone(&target);

    let mut session = MockPluginSession::new();
    session
        .expect_resolved_route_ext_ref()
        .with(eq(0usize))
        .times(1)
        .returning(move |_| Some(std::sync::Arc::clone(&target_for_mock)));
    session
        .expect_start_edgion_plugins_log()
        .returning(|_| EdgionPluginsLogToken::new(0));

    let mut filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "p1".into(),
            resolved_namespace: None,
        },
    );
    filter.set_index(0);

    let mut log = PluginLog::new("test");
    let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
    assert_eq!(result, PluginRunningResult::GoodNext);
}

/// Rule-scope filter with index pointing to a None slot (dangling at resolve
/// time). prepare_execution must push "extension ref not found" log error
/// and return a 500 ErrResponse (sync stage cannot write a body itself, so
/// it signals the status for the response runner to surface). The global
/// store is NOT consulted (verified by the absence of any global store
/// setup in this test).
#[test]
fn index_miss_returns_500() {
    let mut session = MockPluginSession::new();
    session
        .expect_resolved_route_ext_ref()
        .with(eq(7usize))
        .times(1)
        .return_const(None);

    let mut filter = ExtensionRefFilter::new(
        "ns".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "EdgionPlugins".into(),
            name: "p1".into(),
            resolved_namespace: None,
        },
    );
    filter.set_index(7);

    let mut log = PluginLog::new("test");
    let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

/// An ExtensionRef whose `kind` is not `EdgionPlugins` must fail closed.
/// `prepare_execution` returns `Err(())`, which `run_extension_async`
/// (the request stage) maps to a 500 response + JSON body followed by
/// `ErrTerminateRequest` — NOT `GoodNext`.
///
/// Note: the request-stage path returns `ErrTerminateRequest` (after writing
/// a 500 + body itself) rather than `ErrResponse { status: 500 }`, which is
/// what the sync `run_extension` `Err` arm returns. We assert that, plus
/// verify the mock session received the 500 header write, so the test
/// genuinely proves fail-closed behavior.
#[tokio::test]
async fn unsupported_kind_fails_closed() {
    let mut session = MockPluginSession::new();
    session.expect_write_response_header().times(1).returning(|hdr, _eos| {
        assert_eq!(hdr.status.as_u16(), 500);
        Ok(())
    });
    session
        .expect_write_response_body()
        .times(1)
        .returning(|_body, _eos| Ok(()));

    let filter = ExtensionRefFilter::new(
        "default".to_string(),
        LocalObjectReference {
            group: "edgion.io".into(),
            kind: "NotAPlugin".into(),
            name: "x".into(),
            resolved_namespace: None,
        },
    );
    let mut log = PluginLog::with_alias("ExtensionRef", None);
    let result = filter.run_request(&mut session, &mut log).await;
    assert!(
        matches!(result, PluginRunningResult::ErrTerminateRequest),
        "unsupported ExtensionRef kind must fail closed (500 written + ErrTerminateRequest), got {result:?}"
    );
}

#[test]
fn empty_group_does_not_alias_edgion_io() {
    let mut session = MockPluginSession::new();
    let filter = ExtensionRefFilter::new(
        "default".to_string(),
        LocalObjectReference {
            group: String::new(),
            kind: "EdgionPlugins".into(),
            name: "x".into(),
            resolved_namespace: None,
        },
    );
    let mut log = PluginLog::with_alias("ExtensionRef", None);
    assert_eq!(
        filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log),
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        }
    );
}
