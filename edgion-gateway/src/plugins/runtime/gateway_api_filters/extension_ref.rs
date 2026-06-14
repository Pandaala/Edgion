//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::plugins::http::get_global_plugin_store;
use crate::plugins::runtime::log::EdgionPluginsLogToken;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::plugins::runtime::PluginRuntime;
use edgion_resources::filters::{PluginRunningResult, PluginRunningStage};
use edgion_resources::resources::LocalObjectReference;

/// Error response body written when an ExtensionRef cannot be resolved
/// (cycle, depth exceeded, referenced EdgionPlugins not found).
const EXTENSION_REF_ERROR_BODY: &str = r#"{"message":"plugin reference error"}"#;

/// Write a 500 response with a diagnostic JSON body for an async stage that
/// hit a prepare-execution error. Called from `run_extension_async`'s `Err`
/// arm so clients don't see a naked 502 with no log context.
async fn write_extension_ref_error_response(session: &mut dyn PluginSession) {
    match ResponseHeader::build(500, None) {
        Ok(mut hdr) => {
            let _ = hdr.insert_header("Content-Type", "application/json");
            if session.write_response_header(Box::new(hdr), false).await.is_ok() {
                let _ = session
                    .write_response_body(Some(Bytes::from_static(EXTENSION_REF_ERROR_BODY.as_bytes())), true)
                    .await;
            }
        }
        Err(_) => {
            // ResponseHeader::build(500, ...) cannot fail in practice.
            // If the session is already broken, fall through to ErrTerminateRequest
            // and let the runtime surface the termination.
        }
    }
}

/// Filter that handles ExtensionRef to EdgionPlugins
#[derive(Clone)]
pub struct ExtensionRefFilter {
    /// The namespace to look up the plugin (from the HTTPRoute's namespace)
    namespace: String,
    /// The extension reference configuration
    ext_ref: LocalObjectReference,
    /// Position in the rule's ExtensionRef list, assigned at route build time.
    /// `Some(idx)` -> look up resolved target via `session.resolved_route_ext_ref(idx)`.
    /// `None` -> backend-level filter (no per-request resolve cache); fall back to
    /// direct global store lookup. Set to `None` initially; rule-scope builder
    /// patches to `Some` in a later task.
    index: Option<usize>,
}

impl ExtensionRefFilter {
    pub fn new(namespace: String, ext_ref: LocalObjectReference) -> Self {
        Self {
            namespace,
            ext_ref,
            index: None,
        }
    }

    /// Assign the rule-scope index. Called by the rule-scope builder
    /// (`PluginRuntime::add_from_httproute_filters_inner` with assign_indices=true).
    /// No-op when called twice — idempotent on `clone()`-fanout.
    pub(crate) fn set_index(&mut self, index: usize) {
        self.index = Some(index);
    }

    /// Build the lookup key: namespace/name
    fn plugin_key(&self) -> String {
        format!("{}/{}", self.namespace, self.ext_ref.name)
    }

    /// Check if this extension ref points to EdgionPlugins
    fn is_edgion_plugins(&self) -> bool {
        self.ext_ref.kind == "EdgionPlugins" && (self.ext_ref.group.is_empty() || self.ext_ref.group == "edgion.io")
    }

    /// Resolve the referenced plugin runtime without touching PluginLog or
    /// `pending_edgion_plugins_logs`.
    ///
    /// This is the body-stage path: body filters run per chunk and do not
    /// surface PluginLog entries, so calling `start_edgion_plugins_log` here
    /// would push entries that no stage drains, leaking into
    /// `EdgionHttpContext.pending_edgion_plugins_logs` for the entire request
    /// lifetime (one entry per chunk on long streams). This sibling helper
    /// returns just the runtime, with no log side-effects at all.
    fn resolve_runtime_silent(&self, session: &mut dyn PluginSession) -> Option<Arc<PluginRuntime>> {
        if !self.is_edgion_plugins() {
            return None;
        }
        match self.index {
            Some(idx) => session.resolved_route_ext_ref(idx),
            None => get_global_plugin_store().get_plugin_runtime(&self.plugin_key()),
        }
    }

    /// Helper to prepare plugin execution: validation, lookup, logging setup
    /// Returns:
    /// - Ok(Some((plugin, token))): Success
    /// - Ok(None): no longer produced; the `Option` layer is retained to keep this change's diff narrow
    /// - Err(()): Error — unsupported kind, or referenced EdgionPlugins not found; logged
    fn prepare_execution(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<Option<(Arc<PluginRuntime>, EdgionPluginsLogToken)>, ()> {
        if !self.is_edgion_plugins() {
            // An unsupported ExtensionRef kind is a static config error caught
            // at preparse time (`find_invalid_rule_details` /
            // `find_invalid_rule_indices`). This is the runtime backstop — fail
            // closed via `Err(())`, which both callers map to HTTP 500.
            log.push_err("extension ref kind not supported");
            return Err(());
        }

        log.set_refer_to(self.ext_ref.name.clone());

        let key = self.plugin_key();

        // Resolve target: rule-scope reads pre-resolved Vec via index;
        // backend-scope falls back to direct global store lookup.
        let plugin_runtime = match self.index {
            Some(idx) => match session.resolved_route_ext_ref(idx) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    return Err(());
                }
            },
            None => match get_global_plugin_store().get_plugin_runtime(&key) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    return Err(());
                }
            },
        };

        let token = session.start_edgion_plugins_log(self.ext_ref.name.clone());
        Ok(Some((plugin_runtime, token)))
    }

    /// Execute the referenced EdgionPlugins's plugin runtime
    fn run_extension(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let (plugin_runtime, log_token) = match self.prepare_execution(session, log) {
            Ok(Some(res)) => res,
            // Unreachable: prepare_execution never returns Ok(None); see its doc comment.
            Ok(None) => return PluginRunningResult::GoodNext,
            Err(_) => {
                // Sync stage: no `await`, so we cannot write a response body
                // here. Signal the status via `ErrResponse` instead. The
                // response runner (`check_response_stage_terminate`) now turns
                // this into a real local reply: it builds a fresh `500` and
                // `response_filter` writes it to the client, replacing the
                // upstream response (filter-04b). `body` is `None`, so the client
                // gets a bare `500` (no JSON envelope); the request-stage
                // `run_extension_async` path is the one that writes a JSON body.
                // A bare `ErrTerminateRequest` would instead trip the runtime's
                // 502 fail-safe, masking that this is a plugin-reference error.
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
        };

        match stage {
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::with_alias(plugin.name(), plugin.alias());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::Request | PluginRunningStage::UpstreamResponse => {
                // Not executed in sync path
                PluginRunningResult::GoodNext
            }
        }
    }

    /// Async version for stages that require it
    async fn run_extension_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let (plugin_runtime, log_token) = match self.prepare_execution(session, log) {
            Ok(Some(res)) => res,
            // Unreachable: prepare_execution never returns Ok(None); see its doc comment.
            Ok(None) => return PluginRunningResult::GoodNext,
            Err(_) => {
                // Response stage: a direct write + ErrTerminateRequest would be
                // forwarded over by the upstream response (the response runner only
                // aborts on an ErrResponse local-reply, not on ErrTerminateRequest).
                // Return ErrResponse so `check_response_stage_terminate` builds the
                // single replacement reply. (Normally unreachable here because
                // `run_upstream_response` pre-resolves; this guards the TOCTOU window
                // where the ref vanishes between the two lookups.)
                if matches!(stage, PluginRunningStage::UpstreamResponse) {
                    return PluginRunningResult::ErrResponse {
                        status: 500,
                        body: None,
                    };
                }
                // Request stage: without writing a response here, ErrTerminateRequest
                // would surface as a naked 502 (no plugin name in access log, no body)
                // — practically impossible to triage. Write a 500 + JSON body so the
                // terminated request is diagnosable from the client side too.
                write_extension_ref_error_response(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        match stage {
            PluginRunningStage::Request => {
                for plugin in plugin_runtime.request_plugins_iter() {
                    let mut inner_log = PluginLog::with_alias(plugin.name(), plugin.alias());
                    let result = plugin.run_request(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    let mut inner_log = PluginLog::with_alias(plugin.name(), plugin.alias());
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::with_alias(plugin.name(), plugin.alias());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
        }
    }

    /// Run extension ref for body filter stage.
    ///
    /// Resolves silently and dispatches to inner body plugins without any
    /// per-chunk log allocation. See `resolve_runtime_silent` for why.
    fn run_extension_body(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        let plugin_runtime = self.resolve_runtime_silent(session)?;

        let mut max_delay: Option<std::time::Duration> = None;
        for plugin in plugin_runtime.upstream_response_body_plugins_iter() {
            if let Some(d) = plugin.run_upstream_response_body_filter(body, end_of_stream, session) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(d),
                    None => d,
                });
            }
        }

        max_delay
    }
}

#[async_trait]
impl RequestFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.run_extension_async(PluginRunningStage::Request, session, log)
            .await
    }
}

impl UpstreamResponseFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.run_extension(PluginRunningStage::UpstreamResponseFilter, session, log)
    }
}

impl UpstreamResponseBodyFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        self.run_extension_body(body, end_of_stream, session)
    }
}

#[async_trait]
impl UpstreamResponse for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Resolve silently first. The async UpstreamResponse stage is rare, so
        // most referenced bundles have zero async plugins; returning early here
        // avoids the per-response log-token + empty-bundle-log overhead that
        // every ExtensionRef route would otherwise pay. A dangling/unsupported
        // ref maps to `ErrResponse` (NOT a direct write): in the response stage
        // the runner only honors `ErrResponse` as a local-reply short-circuit,
        // so a direct write would be forwarded over by the upstream response.
        match self.resolve_runtime_silent(session) {
            Some(rt) if rt.upstream_response_async_plugins_count() > 0 => {
                self.run_extension_async(PluginRunningStage::UpstreamResponse, session, log)
                    .await
            }
            // Tag the log entry as a reference so the access log records it
            // consistently with the other stages (the runner uses `refer_to` to
            // identify ExtensionRef entries and skip a misleading time_cost).
            Some(_) => {
                log.set_refer_to(self.ext_ref.name.clone());
                PluginRunningResult::GoodNext
            }
            None => {
                log.set_refer_to(self.ext_ref.name.clone());
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::{compile_plugins, get_global_plugin_store};
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::EdgionPlugin;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
    use edgion_resources::resources::http_route::HTTPHeaderFilter;
    use kube::core::ObjectMeta;
    use mockall::predicate::eq;
    use std::collections::HashMap;

    fn make_plugin(key_ns: &str, name: &str) {
        let ep = EdgionPlugins {
            metadata: ObjectMeta {
                namespace: Some(key_ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
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

        let mut map = HashMap::new();
        map.insert(format!("{}/{}", key_ns, name), compile_plugins(&ep));
        get_global_plugin_store().replace_all(map);
    }

    #[test]
    fn test_resolves_via_store_and_runs() {
        use crate::plugins::runtime::log::EdgionPluginsLogToken;

        // Prepare store with plugin
        make_plugin("ns", "p1");

        let mut session = MockPluginSession::new();
        session
            .expect_start_edgion_plugins_log()
            .returning(|_| EdgionPluginsLogToken::new(0));
        session.expect_push_to_edgion_plugins_log().return_const(());

        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::GoodNext);
        // Verify refer_to is set (now just the name string)
        assert!(log.refer_to.is_some());
        assert_eq!(log.refer_to.as_ref().unwrap(), "p1");
    }

    // ============================================
    // Tests for the rule-scope index path (Task 6)
    // ============================================

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
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
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
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
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

    /// Runtime backstop (Task 9): an ExtensionRef whose `kind` is not
    /// `EdgionPlugins` must fail closed. `prepare_execution` returns `Err(())`,
    /// which `run_extension_async` (the request stage) maps to a 500 response +
    /// JSON body followed by `ErrTerminateRequest` — NOT `GoodNext`.
    ///
    /// Note: the request-stage path returns `ErrTerminateRequest` (after
    /// writing a 500 + body itself) rather than `ErrResponse { status: 500 }`.
    /// The draft test in the plan asserted `ErrResponse { status: 500, .. }`,
    /// but that is what the *sync* `run_extension` `Err` arm returns;
    /// `run_request` -> `run_extension_async` returns `ErrTerminateRequest`.
    /// We assert that, plus verify the mock session received the 500 header
    /// write, so the test genuinely proves fail-closed behavior.
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
            },
        );
        let mut log = PluginLog::with_alias("ExtensionRef", None);
        let result = filter.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::ErrTerminateRequest),
            "unsupported ExtensionRef kind must fail closed (500 written + ErrTerminateRequest), got {result:?}"
        );
    }
}
