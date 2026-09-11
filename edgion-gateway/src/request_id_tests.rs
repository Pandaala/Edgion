//! RequestId composition tests through real compiled runtimes and Pingora sessions.
use std::sync::Arc;

use edgion_resources::{EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use crate::plugins::http::compile_plugins;
use crate::plugins::runtime::{PingoraSessionAdapter, PluginRuntime, PluginSession};
use crate::EdgionHttpContext;

fn runtime(config: serde_json::Value) -> Arc<PluginRuntime> {
    let plugin = EdgionPlugin::RequestId(serde_json::from_value(config).unwrap());
    let resource = EdgionPlugins::new(
        "request-id-wiring",
        EdgionPluginsSpec {
            request_plugins: Some(vec![RequestFilterEntry::new(plugin)]),
            ..Default::default()
        },
    );
    compile_plugins(&resource).plugin_runtime.clone()
}

async fn session(extra: &str) -> (Session, DuplexStream, EdgionHttpContext) {
    let (mut client, server) = tokio::io::duplex(8192);
    client
        .write_all(format!("GET / HTTP/1.1\r\nHost: test\r\n{extra}\r\n").as_bytes())
        .await
        .unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    ctx.request_info.trace_id.reconcile_request(session.req_header_mut());
    (session, client, ctx)
}

#[tokio::test]
async fn early_occurrence_is_skipped_later_but_same_route_runtime_executes() {
    let disable = runtime(json!({"defaultId":{"enabled":false}}));
    let enable = runtime(json!({"defaultId":{"enabled":true}}));
    let (mut session, _client, mut ctx) = session("").await;
    disable.run_early_request_ids(&mut session, &mut ctx).await;
    assert!(!session.req_header().headers.contains_key("x-trace-id"));
    enable.run_request_plugins(&mut session, &mut ctx).await;
    assert!(session.req_header().headers.contains_key("x-trace-id"));
    disable
        .run_request_plugins_after_early_ids(&mut session, &mut ctx)
        .await;
    assert!(
        session.req_header().headers.contains_key("x-trace-id"),
        "early entry must not run again in its ordinary occurrence"
    );
    disable.run_request_plugins(&mut session, &mut ctx).await;
    assert!(
        !session.req_header().headers.contains_key("x-trace-id"),
        "the same runtime must execute for a distinct route occurrence"
    );
    assert!(session.req_header().headers.contains_key("traceparent"));
}

#[tokio::test]
async fn compiled_rules_use_current_headers_and_preserve_first_policy() {
    let early = runtime(
        json!({"ids":[{"header":"x-order-id","source":{"type":"header","name":"x-business-id"},"echoResponse":false}]}),
    );
    let route = runtime(json!({"ids":[
        {"header":"x-order-id","generator":{"type":"uuidV4","prefix":"wrong-"}},
        {"header":"x-linked-id","source":{"type":"header","name":"x-order-id"},"generateWhenMissing":false},
        {"header":"x-otel-id","source":{"type":"traceId"},"generateWhenMissing":false}
    ]}));
    let (mut session, _client, mut ctx) = session("x-business-id: business-42\r\n").await;
    early.run_early_request_ids(&mut session, &mut ctx).await;
    route.run_request_plugins(&mut session, &mut ctx).await;
    assert_eq!(session.req_header().headers["x-order-id"], "business-42");
    assert_eq!(session.req_header().headers["x-linked-id"], "business-42");
    assert_eq!(
        session.req_header().headers["x-otel-id"].to_str().unwrap(),
        ctx.request_info.trace_id.standard_trace_id().unwrap()
    );
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("x-order-id", "backend-stale").unwrap();
    ctx.request_info.trace_id.reconcile_response(&mut response);
    assert!(!response.headers.contains_key("x-order-id"));
    assert_eq!(response.headers["x-linked-id"], "business-42");
}

#[tokio::test]
async fn direct_error_response_cannot_resurrect_disabled_default() {
    let disable =
        runtime(json!({"defaultId":{"enabled":false},"ids":[{"header":"x-error-id","source":{"type":"traceId"}}]}));
    let (mut session, mut client, mut ctx) = session("x-trace-id: client-id\r\n").await;
    disable.run_early_request_ids(&mut session, &mut ctx).await;
    let expected = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
    let mut response = ResponseHeader::build(403, None).unwrap();
    response.insert_header("content-length", "0").unwrap();
    response.insert_header("x-trace-id", "backend-stale").unwrap();
    response.insert_header("x-error-id", "backend-stale").unwrap();
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.set_response_header("x-trace-id", "queued-stale").unwrap();
        adapter.write_response_header(Box::new(response), true).await.unwrap();
    }
    let mut bytes = [0; 4096];
    let count = tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let wire = std::str::from_utf8(&bytes[..count]).unwrap().to_ascii_lowercase();
    assert!(wire.starts_with("http/1.1 403"));
    assert!(!wire.contains("x-trace-id:"));
    assert!(wire.contains(&format!("x-error-id: {expected}")));
}

#[tokio::test]
async fn inherited_target_wins_and_duplicates_fall_back_to_standard_source() {
    let rule =
        runtime(json!({"ids":[{"header":"x-custom-id","source":{"type":"traceId"},"generateWhenMissing":false}]}));
    for (extra, expected_inbound) in [
        ("x-custom-id: inherited\r\n", Some("inherited")),
        ("x-custom-id: one\r\nx-custom-id: two\r\n", None),
    ] {
        let (mut session, _client, mut ctx) = session(extra).await;
        rule.run_request_plugins(&mut session, &mut ctx).await;
        let expected = expected_inbound.unwrap_or_else(|| ctx.request_info.trace_id.standard_trace_id().unwrap());
        assert_eq!(session.req_header().headers["x-custom-id"].to_str().unwrap(), expected);
        assert_eq!(session.req_header().headers.get_all("x-custom-id").iter().count(), 1);
    }
}

#[test]
fn reserved_default_is_rejected_by_schema_and_gateway_runtime() {
    for header in ["x-trace-id", "X-Trace-ID"] {
        let plugin = EdgionPlugin::RequestId(serde_json::from_value(json!({"ids":[{"header":header}]})).unwrap());
        let resource = EdgionPlugins::new(
            "reserved-id",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(plugin)]),
                ..Default::default()
            },
        );
        assert!(!resource.validate_plugins().is_empty());
        let compiled = compile_plugins(&resource);
        assert!(!compiled.preparse_errors.is_empty());
        assert!(!compiled.is_activatable);
    }
}
