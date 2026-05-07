//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::core::gateway::plugins::http::get_global_plugin_store;
use crate::core::gateway::plugins::runtime::log::EdgionPluginsLogToken;
use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{
    PluginSession, RequestFilter, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::core::gateway::plugins::runtime::PluginRuntime;
use crate::types::filters::{PluginRunningResult, PluginRunningStage};
use crate::types::resources::LocalObjectReference;

/// Error response body written when an ExtensionRef cannot be resolved
/// (cycle, depth exceeded, referenced EdgionPlugins not found).
const EXTENSION_REF_ERROR_BODY: &[u8] = br#"{"message":"plugin reference error"}"#;

/// Write a 500 response with a diagnostic JSON body for an async stage that
/// hit a prepare-execution error. Called from `run_extension_async`'s `Err`
/// arm so clients don't see a naked 502 with no log context.
async fn write_extension_ref_error_response(session: &mut dyn PluginSession) {
    match ResponseHeader::build(500, None) {
        Ok(mut hdr) => {
            let _ = hdr.insert_header("Content-Type", "application/json");
            if session.write_response_header(Box::new(hdr), false).await.is_ok() {
                let _ = session
                    .write_response_body(Some(Bytes::from_static(EXTENSION_REF_ERROR_BODY)), true)
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

/// Default maximum allowed nested plugin references to avoid infinite loops
pub const DEFAULT_PLUGIN_REF_DEPTH: usize = 5;

/// Filter that handles ExtensionRef to EdgionPlugins
#[derive(Clone)]
pub struct ExtensionRefFilter {
    /// The namespace to look up the plugin (from the HTTPRoute's namespace)
    namespace: String,
    /// The extension reference configuration
    ext_ref: LocalObjectReference,
    /// Max depth allowed for this filter instance
    max_depth: usize,
    /// Position in the rule's ExtensionRef list, assigned at route build time.
    /// `Some(idx)` -> look up resolved target via `session.resolved_route_ext_ref(idx)`.
    /// `None` -> backend-level filter (no per-request resolve cache); fall back to
    /// direct global store lookup. Set to `None` initially; rule-scope builder
    /// patches to `Some` in a later task.
    index: Option<usize>,
}

impl ExtensionRefFilter {
    pub fn new(namespace: String, ext_ref: LocalObjectReference, max_depth: usize) -> Self {
        Self {
            namespace,
            ext_ref,
            max_depth,
            index: None,
        }
    }

    /// Assign the rule-scope index. Called by the rule-scope builder
    /// (`PluginRuntime::add_from_httproute_filters_inner` with assign_indices=true).
    /// No-op when called twice — idempotent on `clone()`-fanout.
    pub(crate) fn set_index(&mut self, index: usize) {
        self.index = Some(index);
    }

    #[inline]
    fn finish(session: &mut dyn PluginSession, result: PluginRunningResult) -> PluginRunningResult {
        session.pop_plugin_ref();
        result
    }

    /// Build the lookup key: namespace/name
    fn plugin_key(&self) -> String {
        format!("{}/{}", self.namespace, self.ext_ref.name)
    }

    /// Check if this extension ref points to EdgionPlugins
    fn is_edgion_plugins(&self) -> bool {
        self.ext_ref.kind == "EdgionPlugins" && (self.ext_ref.group.is_empty() || self.ext_ref.group == "edgion.io")
    }

    /// Helper to prepare plugin execution: validation, lookup, logging setup
    /// Returns:
    /// - Ok(Some((plugin, token))): Success, session pushed (must be popped)
    /// - Ok(None): Skip (not EdgionPlugins), no session modification
    /// - Err(()): Error (cycle/depth/not found), logged, session clean (popped if needed)
    fn prepare_execution(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<Option<(Arc<PluginRuntime>, EdgionPluginsLogToken)>, ()> {
        if !self.is_edgion_plugins() {
            log.push_err("extension ref kind not supported");
            return Ok(None);
        }

        log.set_refer_to(self.ext_ref.name.clone());

        let key = self.plugin_key();

        if session.has_plugin_ref(&key) {
            log.push_err("cyclic reference");
            return Err(());
        }
        if session.plugin_ref_depth() >= self.max_depth {
            log.push_err("extension ref depth exceeded");
            return Err(());
        }

        session.push_plugin_ref(key.clone());

        // Resolve target: rule-scope reads pre-resolved Vec via index;
        // backend-scope falls back to direct global store lookup.
        let plugin_runtime = match self.index {
            Some(idx) => match session.resolved_route_ext_ref(idx) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    session.pop_plugin_ref();
                    return Err(());
                }
            },
            None => match get_global_plugin_store().get_plugin_runtime(&key) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    session.pop_plugin_ref();
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
            Ok(None) => return PluginRunningResult::GoodNext,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };

        match stage {
            PluginRunningStage::Request => {
                log.push_err("sync path used");
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponse => {
                // Not executed in sync path
                Self::finish(session, PluginRunningResult::GoodNext)
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
            Ok(None) => return PluginRunningResult::GoodNext,
            Err(_) => {
                // Without writing a response here, ErrTerminateRequest would surface
                // as a naked 502 (no plugin name in access log, no body) — practically
                // impossible to triage. Write a 500 + JSON body so the terminated
                // request is diagnosable from the client side too.
                write_extension_ref_error_response(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        match stage {
            PluginRunningStage::Request => {
                for plugin in plugin_runtime.request_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_request(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    let mut inner_log = PluginLog::new(plugin.name());
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    session.push_to_edgion_plugins_log(&log_token, inner_log);

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return Self::finish(session, result);
                    }
                }
                Self::finish(session, PluginRunningResult::GoodNext)
            }
        }
    }

    /// Run extension ref for body filter stage
    fn run_extension_body(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Option<std::time::Duration> {
        let (plugin_runtime, log_token) = match self.prepare_execution(session, log) {
            Ok(Some(res)) => res,
            Ok(None) => return None,
            Err(_) => return None,
        };

        let mut max_delay: Option<std::time::Duration> = None;

        // Wrap body filter loop in catch_unwind so pop_plugin_ref runs
        // even if a plugin panics. Without this, a panic would leave a
        // stale entry on plugin_ref_stack, causing false cycle detection
        // on subsequent chunks for the same session.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for plugin in plugin_runtime.upstream_response_body_plugins_iter() {
                let mut inner_log = PluginLog::new(plugin.name());
                let delay = plugin.run_upstream_response_body_filter(body, end_of_stream, session, &mut inner_log);
                session.push_to_edgion_plugins_log(&log_token, inner_log);

                if let Some(d) = delay {
                    max_delay = Some(match max_delay {
                        Some(current) => current.max(d),
                        None => d,
                    });
                }
            }
        }));

        session.pop_plugin_ref();

        match result {
            Ok(()) => max_delay,
            // pop_plugin_ref above ensured cleanup; re-throw the panic so
            // Pingora's worker-level handler aborts the request.
            Err(payload) => std::panic::resume_unwind(payload),
        }
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
        log: &mut PluginLog,
    ) -> Option<std::time::Duration> {
        self.run_extension_body(body, end_of_stream, session, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::http::get_global_plugin_store;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::EdgionPlugin;
    use crate::types::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
    use crate::types::resources::http_route::HTTPHeaderFilter;
    use kube::core::ObjectMeta;
    use mockall::predicate::eq;
    use std::collections::HashMap;

    fn make_plugin(key_ns: &str, name: &str) {
        let mut ep = EdgionPlugins {
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
        ep.preparse();

        let mut map = HashMap::new();
        map.insert(format!("{}/{}", key_ns, name), ep);
        get_global_plugin_store().replace_all(map);
    }

    #[test]
    fn test_cycle_detected() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(true);
        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_depth_exceeded() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(DEFAULT_PLUGIN_REF_DEPTH);
        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_depth_within_limit_runs() {
        use crate::core::gateway::plugins::runtime::log::EdgionPluginsLogToken;

        // Prepare store with plugin
        make_plugin("ns", "p1");

        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(0usize);
        session.expect_push_plugin_ref().return_const(());
        session.expect_pop_plugin_ref().return_const(());
        session
            .expect_start_edgion_plugins_log()
            .returning(|_| EdgionPluginsLogToken::new(0, 0));
        session.expect_push_to_edgion_plugins_log().return_const(());

        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::GoodNext);
        // Verify refer_to is set (now just the name string)
        assert!(log.refer_to.is_some());
        assert_eq!(log.refer_to.as_ref().unwrap(), "p1");
    }

    /// The async Request stage must write a 500 body when a cycle is detected,
    /// instead of returning a naked `ErrTerminateRequest` that surfaces as a blank 502 to the client.
    #[tokio::test]
    async fn test_cycle_writes_500_body_in_request_stage() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(true);
        // The new Err branch must call write_response_header + write_response_body.
        session.expect_write_response_header().times(1).returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .times(1)
            .withf(|body, end| {
                end == &true
                    && body
                        .as_ref()
                        .map(|b| b.as_ref() == EXTENSION_REF_ERROR_BODY)
                        .unwrap_or(false)
            })
            .returning(|_, _| Ok(()));

        let filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        let mut log = PluginLog::new("test");
        let result = filter
            .run_extension_async(PluginRunningStage::Request, &mut session, &mut log)
            .await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
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
        use crate::core::gateway::plugins::runtime::PluginRuntime;

        let target = std::sync::Arc::new(PluginRuntime::new());
        let target_for_mock = std::sync::Arc::clone(&target);

        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(0usize);
        session.expect_push_plugin_ref().return_const(());
        session
            .expect_resolved_route_ext_ref()
            .with(eq(0usize))
            .times(1)
            .returning(move |_| Some(std::sync::Arc::clone(&target_for_mock)));
        session
            .expect_start_edgion_plugins_log()
            .returning(|_| EdgionPluginsLogToken::new(0, 0));
        session.expect_pop_plugin_ref().return_const(());

        let mut filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        filter.set_index(0);

        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    /// Rule-scope filter with index pointing to a None slot (dangling at resolve
    /// time). prepare_execution must push "extension ref not found" log error,
    /// pop_plugin_ref, and return ErrTerminateRequest. The global store is NOT
    /// consulted (verified by the absence of any global store setup in this test).
    #[test]
    fn index_miss_terminates_request() {
        let mut session = MockPluginSession::new();
        session.expect_has_plugin_ref().with(eq("ns/p1")).return_const(false);
        session.expect_plugin_ref_depth().return_const(0usize);
        session.expect_push_plugin_ref().return_const(());
        session
            .expect_resolved_route_ext_ref()
            .with(eq(7usize))
            .times(1)
            .return_const(None);
        session.expect_pop_plugin_ref().return_const(());

        let mut filter = ExtensionRefFilter::new(
            "ns".to_string(),
            LocalObjectReference {
                group: "".into(),
                kind: "EdgionPlugins".into(),
                name: "p1".into(),
            },
            DEFAULT_PLUGIN_REF_DEPTH,
        );
        filter.set_index(7);

        let mut log = PluginLog::new("test");
        let result = filter.run_extension(PluginRunningStage::UpstreamResponseFilter, &mut session, &mut log);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }
}
