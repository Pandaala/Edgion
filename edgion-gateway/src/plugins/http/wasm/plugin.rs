//! Wasm plugin — executes proxy-wasm guests at the request-header and
//! response-header phases. See mod.rs docs for scope and limitations.

use std::collections::HashMap;

use async_trait::async_trait;

use std::sync::Arc;

use super::callout::execute_callout;
use super::host_abi::{HeaderOp, LocalResponse, WasmPhase, MAP_HTTP_REQUEST_HEADERS, MAP_HTTP_RESPONSE_HEADERS};
use super::vm_manager::{global_vm_manager, runtime_cache_key, VmLease};
use crate::observe::metrics::registry::record_wasm_fail_mode;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{
    PluginLog, PluginSession, RequestFilter, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::WasmConfig;

const PLUGIN_NAME: &str = "Wasm";

pub struct WasmPlugin {
    name: String,
    config: WasmConfig,
    /// Runtime cache key (schema key + file stat for `file` sources),
    /// computed once at build so every path of this plugin instance —
    /// preload, async load, sync `try_get`, refcount — agrees on one key
    /// even if the source file is replaced on disk mid-lifetime.
    cache_key: String,
    /// Bounded-cardinality source descriptor for metrics.
    metrics_label: String,
    /// Normalized callout allowlist origins, shared into each phase's
    /// `HostState` via `arm_dispatch`. Empty = callouts disabled.
    callout_allowlist: Arc<[String]>,
}

impl WasmPlugin {
    fn build(config: &WasmConfig) -> Self {
        let cache_key = runtime_cache_key(config);
        // Admission validation already rejects malformed entries; a residual
        // bad entry here (e.g. a file-mode gateway without the controller
        // validation) is dropped with a WARN rather than failing the plugin.
        let callout_allowlist: Vec<String> = config
            .callout_allowlist
            .iter()
            .filter_map(
                |raw| match edgion_resources::plugin_configs::normalize_callout_origin(raw) {
                    Ok(o) => Some(o),
                    Err(e) => {
                        tracing::warn!(error = %e, "wasm calloutAllowlist entry dropped");
                        None
                    }
                },
            )
            .collect();
        let plugin = WasmPlugin {
            name: PLUGIN_NAME.to_string(),
            config: config.clone(),
            cache_key: cache_key.clone(),
            metrics_label: config.source.metrics_label(),
            callout_allowlist: Arc::from(callout_allowlist),
        };
        // Pin the cache entry for this plugin's lifetime (released in Drop;
        // the manager evicts the entry after a grace period once no plugin
        // references it anymore).
        global_vm_manager().register_plugin(&plugin.cache_key);
        // Warm the module cache so the first request does not pay compile
        // latency (best-effort; lazy load covers the miss).
        global_vm_manager().spawn_preload(config, cache_key);
        plugin
    }

    pub fn create_request(config: &WasmConfig) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self::build(config)))
    }

    pub fn create_response_filter(config: &WasmConfig) -> Option<Box<dyn UpstreamResponseFilter>> {
        Some(Box::new(Self::build(config)))
    }

    pub fn create_body_filter(config: &WasmConfig) -> Option<Box<dyn UpstreamResponseBodyFilter>> {
        Some(Box::new(Self::build(config)))
    }

    fn fail_mode(&self, log: &mut PluginLog, reason: &str) -> PluginRunningResult {
        if self.config.fail_open {
            record_wasm_fail_mode(&self.metrics_label, reason, "open");
            log.push_fmt("wasm {}:fail-open", la![reason]);
            PluginRunningResult::GoodNext
        } else {
            record_wasm_fail_mode(&self.metrics_label, reason, "close");
            log.push_fmt("wasm {}:fail-close", la![reason]);
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some("wasm plugin error".to_string()),
            }
        }
    }

    fn properties(&self, session: &dyn PluginSession, phase: WasmPhase) -> HashMap<String, String> {
        let mut p = HashMap::new();
        p.insert("request.path".to_string(), session.get_path().to_string());
        p.insert("request.method".to_string(), session.get_method().to_string());
        p.insert(
            "request.host".to_string(),
            session.header_value("host").unwrap_or_default(),
        );
        p.insert("source.address".to_string(), session.remote_addr().to_string());
        p.insert("plugin_name".to_string(), PLUGIN_NAME.to_string());
        if phase == WasmPhase::ResponseHeaders {
            if let Some(code) = session.get_response_status() {
                p.insert("response.code".to_string(), code.to_string());
            }
        }
        p
    }

    /// Response-headers execution once a lease is held (sync stage — no
    /// callout support: dispatch is not armed, so the guest sees
    /// UNIMPLEMENTED). Returns the plugin result after draining ops / local
    /// response onto the session.
    fn run_leased_response(
        &self,
        mut lease: VmLease,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let request_headers = session.request_headers();
        let response_headers = session.response_headers();
        let num_headers = response_headers.len() as i32;
        let props = self.properties(session, WasmPhase::ResponseHeaders);
        lease
            .state_mut()
            .begin_phase(WasmPhase::ResponseHeaders, request_headers, response_headers, props);

        let action = match lease.call_on_response_headers(num_headers) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, "wasm guest trap");
                return self.fail_mode(log, "trap");
            }
        };
        self.finish_phase(lease, session, log, action, false)
    }

    /// Request-headers execution with callout + Pause support. The context
    /// stays open across callout delivery so the paused guest can receive
    /// `proxy_on_http_call_response` and then resume (continue_request) or
    /// short-circuit (send_local_response).
    async fn run_leased_request(
        &self,
        mut lease: VmLease,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let request_headers = session.request_headers();
        let num_headers = request_headers.len() as i32;
        let props = self.properties(session, WasmPhase::RequestHeaders);
        lease
            .state_mut()
            .begin_phase(WasmPhase::RequestHeaders, request_headers, Vec::new(), props);
        lease
            .state_mut()
            .arm_dispatch(self.callout_allowlist.clone(), self.config.callout_timeout_ms);

        let (ctx_id, action) = match lease.call_on_request_headers_open(num_headers) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "wasm guest trap");
                return self.fail_mode(log, "trap");
            }
        };

        let paused = action != 0;
        let had_callouts = !lease.state().pending_callouts.is_empty();
        if paused && had_callouts {
            if let Err(e) = self.resolve_callouts(&mut lease, ctx_id, log).await {
                tracing::warn!(error = %e, "wasm callout resolution failed");
                return self.fail_mode(log, "callout");
            }
        } else if !paused && had_callouts {
            // Fire-and-forget dispatch without Pause is not supported: the
            // lease returns to the pool when this call ends, so there is no
            // execution context left to deliver the callback on. Documented
            // v1 semantics: dispatch must be followed by Action::Pause.
            let dropped = lease.state_mut().pending_callouts.len();
            lease.state_mut().pending_callouts.clear();
            log.push_fmt("wasm callouts-dropped={}", la![dropped.to_string().as_str()]);
        }

        if lease.close_context(ctx_id).is_err() {
            return self.fail_mode(log, "trap");
        }

        // A paused guest that neither continued nor answered locally after
        // all callouts completed gets the fail-mode treatment; a pause
        // without any callout keeps the v1 "pause-ignored" behavior.
        let state = lease.state();
        if paused && had_callouts && !state.continue_requested && state.local_response.is_none() {
            drop(lease);
            return self.fail_mode(log, "no-resume");
        }
        self.finish_phase(lease, session, log, action, had_callouts)
    }

    /// Execute queued callouts and deliver each result to the paused guest.
    /// Bounded: per-request dispatch cap (enforced at dispatch time) plus a
    /// round cap here (a callback may dispatch follow-up callouts). Stops
    /// early once the guest resumes or answers locally.
    async fn resolve_callouts(&self, lease: &mut VmLease, ctx_id: i32, log: &mut PluginLog) -> Result<(), String> {
        const MAX_ROUNDS: usize = 4;
        let mut delivered = 0usize;
        for _round in 0..MAX_ROUNDS {
            let pending = std::mem::take(&mut lease.state_mut().pending_callouts);
            if pending.is_empty() {
                break;
            }
            for callout in pending {
                if lease.state().continue_requested || lease.state().local_response.is_some() {
                    // The guest already decided; remaining callouts are moot.
                    break;
                }
                let response = execute_callout(&callout, self.config.callout_timeout_ms, &self.metrics_label).await;
                let num_headers = response.headers.len() as i32;
                let body_size = response.body.len() as i32;
                let token = response.token;
                lease.state_mut().callout_response = Some(response);
                lease.call_on_http_call_response(ctx_id, token, num_headers, body_size)?;
                lease.state_mut().callout_response = None;
                delivered += 1;
            }
            if lease.state().continue_requested || lease.state().local_response.is_some() {
                break;
            }
        }
        // Anything still pending after the round cap is dropped (guest is
        // dispatching in a loop); the no-resume fallback in the caller
        // decides the request's fate.
        let leftover = lease.state_mut().pending_callouts.len();
        lease.state_mut().pending_callouts.clear();
        log.push_fmt(
            "wasm callouts={} leftover={}",
            la![delivered.to_string().as_str(), leftover.to_string().as_str()],
        );
        Ok(())
    }

    /// Drain guest results (header ops, local response, logs) onto the
    /// session and translate the guest action into a plugin result.
    fn finish_phase(
        &self,
        mut lease: VmLease,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        action: i32,
        had_callouts: bool,
    ) -> PluginRunningResult {
        // Drain results out of the store before the lease returns to the pool.
        let state = lease.state_mut();
        let ops = std::mem::take(&mut state.pending_ops);
        let local = state.local_response.take();
        let guest_logs = state.log_lines.len();
        drop(lease);

        let op_count = ops.len();
        apply_ops(session, ops);

        if let Some(local) = local {
            return finish_local_response(session, log, local);
        }
        if action != 0 && !had_callouts {
            // Pause without a callout cannot be honored (nothing to wait for).
            log.push_fmt("wasm ok ops={} pause-ignored", la![op_count.to_string().as_str()]);
        } else {
            log.push_fmt(
                "wasm ok ops={} logs={}",
                la![op_count.to_string().as_str(), guest_logs.to_string().as_str()],
            );
        }
        PluginRunningResult::GoodNext
    }
}

impl UpstreamResponseBodyFilter for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    /// Read-only per-chunk observation of the response body via
    /// `proxy_on_http_response_body` + `get_buffer_bytes(HttpResponseBody)`.
    ///
    /// Constraints (by design of this stage, not implementation shortcuts):
    /// - the guest sees the CURRENT chunk only (no cross-chunk buffering);
    /// - the body cannot be modified and the response cannot be
    ///   short-circuited (headers are already sent) — a guest verdict can
    ///   only be logged (proxy_log) or aggregated via shared state;
    /// - Pause is meaningless in the read-only form and is ignored;
    /// - a trap never interrupts the stream (failOpen-like, regardless of
    ///   `failOpen`): the lease is discarded and streaming continues.
    ///
    /// Each chunk runs under a reduced CPU budget (`timeoutMs / 10`) so a
    /// slow guest cannot stall a long stream chunk by chunk.
    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        _session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        let Some(entry) = global_vm_manager().try_get(&self.cache_key) else {
            global_vm_manager().spawn_preload(&self.config, self.cache_key.clone());
            return None;
        };
        // Header-only guests pay nothing per chunk: skip the lease entirely
        // when the module has no body hook (probed once at load).
        if !entry.has_body_hook() {
            return None;
        }
        let chunk_budget_ms = (self.config.timeout_ms / 10).max(10);
        let mut lease = match entry.acquire(chunk_budget_ms) {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!(error = %e, "wasm body observation: instance acquire failed");
                return None;
            }
        };
        let state = lease.state_mut();
        // Lean phase begin: body observation needs no header snapshot or
        // property map (per-chunk cost discipline).
        state.begin_phase(WasmPhase::ResponseBody, Vec::new(), Vec::new(), HashMap::new());
        state.body_chunk = body.as_ref().map(|b| b.to_vec()).unwrap_or_default();
        let size = state.body_chunk.len() as i32;
        if let Err(e) = lease.call_on_response_body(size, end_of_stream) {
            tracing::debug!(error = %e, "wasm response-body hook failed (stream continues)");
        }
        None
    }
}

impl Drop for WasmPlugin {
    fn drop(&mut self) {
        // Release this plugin's pin on the module cache entry. PluginStore
        // replaces plugin instances wholesale on reload, so a config change
        // drops the old instance here and the manager evicts the now-orphaned
        // entry after its grace period.
        global_vm_manager().unregister_plugin(&self.cache_key, &self.metrics_label);
    }
}

fn apply_ops(session: &mut dyn PluginSession, ops: Vec<HeaderOp>) {
    for op in ops {
        let result = match op {
            HeaderOp::Set {
                map: MAP_HTTP_REQUEST_HEADERS,
                name,
                value,
            } => session.set_request_header(&name, &value),
            HeaderOp::Add {
                map: MAP_HTTP_REQUEST_HEADERS,
                name,
                value,
            } => session.append_request_header(&name, &value),
            HeaderOp::Remove {
                map: MAP_HTTP_REQUEST_HEADERS,
                name,
            } => session.remove_request_header(&name),
            HeaderOp::Set {
                map: MAP_HTTP_RESPONSE_HEADERS,
                name,
                value,
            } => session.set_response_header(&name, &value),
            HeaderOp::Add {
                map: MAP_HTTP_RESPONSE_HEADERS,
                name,
                value,
            } => session.append_response_header(&name, &value),
            HeaderOp::Remove {
                map: MAP_HTTP_RESPONSE_HEADERS,
                name,
            } => session.remove_response_header(&name),
            HeaderOp::ReplaceAll { map, pairs } => {
                for (name, value) in pairs {
                    let _ = match map {
                        MAP_HTTP_REQUEST_HEADERS => session.set_request_header(&name, &value),
                        MAP_HTTP_RESPONSE_HEADERS => session.set_response_header(&name, &value),
                        _ => Ok(()),
                    };
                }
                Ok(())
            }
            _ => Ok(()), // unknown map id: ignore
        };
        if let Err(e) = result {
            tracing::debug!(error = %e, "wasm header op apply failed");
        }
    }
}

fn finish_local_response(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    local: LocalResponse,
) -> PluginRunningResult {
    for (name, value) in &local.headers {
        let _ = session.set_response_header(name, value);
    }
    log.push_fmt("wasm local {}", la![local.status.to_string().as_str()]);
    let body = (!local.body.is_empty()).then(|| String::from_utf8_lossy(&local.body).into_owned());
    PluginRunningResult::ErrResponse {
        status: local.status,
        body,
    }
}

#[async_trait]
impl RequestFilter for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let entry = match global_vm_manager().get_or_load(&self.config, &self.cache_key).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "wasm module unavailable");
                return self.fail_mode(log, "load");
            }
        };
        let lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "wasm instance acquire failed");
                return self.fail_mode(log, "acquire");
            }
        };
        self.run_leased_request(lease, session, log).await
    }
}

impl UpstreamResponseFilter for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Sync hook: no await available. Use the non-blocking cache lookup;
        // on miss kick a background load and apply the fail mode this time.
        let Some(entry) = global_vm_manager().try_get(&self.cache_key) else {
            global_vm_manager().spawn_preload(&self.config, self.cache_key.clone());
            return self.fail_mode(log, "not loaded");
        };
        let lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "wasm instance acquire failed");
                return self.fail_mode(log, "acquire");
            }
        };
        self.run_leased_response(lease, session, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::PluginLog;

    /// Guest whose request-headers hook sets `x-wat-added: yes`, and whose
    /// response-headers hook sends a 403 local response with body "denied"
    /// (the response-headers hook is unused by these request-phase tests).
    ///
    /// Duplicated from `host_abi.rs`'s test-only `GUEST` constant: that
    /// constant lives inside `host_abi`'s own `#[cfg(test)] mod tests` and is
    /// not visible here.
    const GUEST_SET_HEADER: &str = r#"
    (module
      (import "env" "proxy_replace_header_map_value"
        (func $set_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $slr (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 4096))
      ;; offsets: 0: "x-wat-added" (11), 16: "yes" (3), 32: "denied" (6)
      (data (i32.const 0) "x-wat-added")
      (data (i32.const 16) "yes")
      (data (i32.const 32) "denied")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (drop (call $set_hdr (i32.const 0) (i32.const 0) (i32.const 11) (i32.const 16) (i32.const 3)))
        (i32.const 0))
      (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
        (drop (call $slr (i32.const 403) (i32.const 0) (i32.const 0)
                         (i32.const 32) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
        (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// Guest whose request-headers hook hits `unreachable` (simulated panic).
    ///
    /// Duplicated from `vm_manager.rs`'s test-only `TRAP_GUEST` constant for
    /// the same visibility reason as `GUEST_SET_HEADER` above.
    const GUEST_TRAP: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) unreachable)
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// Callout demo guest: on request headers it reads the target origin
    /// from the `x-callout-url` request header, dispatches a GET /check
    /// callout to it, and returns Pause. On the callout response it resumes
    /// the request when the callout completed (num_headers > 0) or sends a
    /// 403 local response when the callout failed. A dispatch rejection
    /// (denied origin) is answered immediately with a 503 local response.
    ///
    /// The embedded 69-byte data blob at offset 32 is the proxy-wasm
    /// serialized header map for [(":method","GET"), (":path","/check"),
    /// (":authority","test")].
    const GUEST_CALLOUT: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_dispatch_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_continue_request" (func $cont (result i32)))
      (import "env" "proxy_send_local_response"
        (func $slr (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "x-callout-url")
      (data (i32.const 32) "\03\00\00\00\07\00\00\00\03\00\00\00\05\00\00\00\06\00\00\00\0a\00\00\00\04\00\00\00:method\00GET\00:path\00/check\00:authority\00test\00")
      (data (i32.const 128) "denied")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (if (i32.ne (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 13)
                              (i32.const 256) (i32.const 260)) (i32.const 0))
          (then
            (drop (call $slr (i32.const 500) (i32.const 0) (i32.const 0)
                             (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
            (return (i32.const 0))))
        (if (i32.ne (call $dispatch (i32.load (i32.const 256)) (i32.load (i32.const 260))
                              (i32.const 32) (i32.const 69)
                              (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
                              (i32.const 1000) (i32.const 264)) (i32.const 0))
          (then
            (drop (call $slr (i32.const 503) (i32.const 0) (i32.const 0)
                             (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
            (return (i32.const 0))))
        (i32.const 1))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32)
        (if (i32.gt_s (local.get 2) (i32.const 0))
          (then (drop (call $cont)))
          (else (drop (call $slr (i32.const 403) (i32.const 0) (i32.const 0)
                                 (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0))))))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// Same dispatch flow as `GUEST_CALLOUT`, but the callout response
    /// handler neither resumes nor answers — exercising the host's
    /// no-resume fail-mode fallback.
    const GUEST_CALLOUT_NO_RESUME: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_dispatch_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "x-callout-url")
      (data (i32.const 32) "\03\00\00\00\07\00\00\00\03\00\00\00\05\00\00\00\06\00\00\00\0a\00\00\00\04\00\00\00:method\00GET\00:path\00/check\00:authority\00test\00")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (drop (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 13)
                             (i32.const 256) (i32.const 260)))
        (drop (call $dispatch (i32.load (i32.const 256)) (i32.load (i32.const 260))
                              (i32.const 32) (i32.const 69)
                              (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
                              (i32.const 1000) (i32.const 264)))
        (i32.const 1))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    fn write_temp_wasm(wat: &str) -> tempfile::NamedTempFile {
        let bytes = wat::parse_str(wat).expect("wat parses");
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), &bytes).unwrap();
        f
    }

    fn config_for(path: &std::path::Path, fail_open: bool) -> WasmConfig {
        let mut c = WasmConfig::default();
        c.source.file = Some(path.to_string_lossy().into_owned());
        c.fail_open = fail_open;
        c
    }

    fn base_mock() -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_request_headers()
            .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
        s.expect_get_path().return_const("/x".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_remote_addr().return_const("1.2.3.4".to_string());
        s
    }

    #[tokio::test]
    async fn request_phase_applies_header_set() {
        let f = write_temp_wasm(GUEST_SET_HEADER);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = base_mock();
        session
            .expect_set_request_header()
            .withf(|k, v| k == "x-wat-added" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn trap_fail_closed_returns_500() {
        let f = write_temp_wasm(GUEST_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).unwrap();
        let mut session = base_mock();
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[tokio::test]
    async fn trap_fail_open_continues() {
        let f = write_temp_wasm(GUEST_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).unwrap();
        let mut session = base_mock();
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn gateway_survives_trap_then_serves_next_request() {
        let f = write_temp_wasm(GUEST_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).unwrap();
        for _ in 0..3 {
            let mut session = base_mock();
            let mut log = PluginLog::new("Wasm");
            let _ = plugin.run_request(&mut session, &mut log).await;
        }
        // reaching here without panic = recovery works
    }

    #[tokio::test]
    async fn load_failure_fail_closed() {
        let mut c = WasmConfig::default();
        c.source.file = Some("/nonexistent/missing.wasm".to_string());
        let plugin = WasmPlugin::create_request(&c).unwrap();
        let mut session = base_mock();
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    /// Mock session for the callout guests: carries the callout target in
    /// the `x-callout-url` request header.
    fn callout_mock(target: &str) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        let target = target.to_string();
        s.expect_request_headers().returning(move || {
            vec![
                ("host".to_string(), "t.example.com".to_string()),
                ("x-callout-url".to_string(), target.clone()),
            ]
        });
        s.expect_get_path().return_const("/x".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_remote_addr().return_const("1.2.3.4".to_string());
        s
    }

    fn callout_config(path: &std::path::Path, allowlist: Vec<String>, fail_open: bool) -> WasmConfig {
        let mut c = config_for(path, fail_open);
        c.callout_allowlist = allowlist;
        c
    }

    /// Callout happy path: guest pauses on a dispatched callout, the target
    /// answers 200, guest resumes → request proceeds. Verifies the callout
    /// actually hit the target with the guest-supplied path and authority.
    #[tokio::test]
    async fn callout_success_resumes_request() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .and(header("host", "test"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
            .mount(&server)
            .await;

        let f = write_temp_wasm(GUEST_CALLOUT);
        let config = callout_config(f.path(), vec![server.uri()], false);
        let plugin = WasmPlugin::create_request(&config).unwrap();
        let mut session = callout_mock(&server.uri());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// Callout target unreachable → failure callback (0 headers) → guest
    /// sends a 403 local response.
    #[tokio::test]
    async fn callout_failure_guest_denies() {
        let f = write_temp_wasm(GUEST_CALLOUT);
        // Port 1 is unreachable; the origin is allowlisted so dispatch
        // succeeds and the failure surfaces via the callback.
        let target = "http://127.0.0.1:1";
        let config = callout_config(f.path(), vec![target.to_string()], false);
        let plugin = WasmPlugin::create_request(&config).unwrap();
        let mut session = callout_mock(target);
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(
            matches!(res, PluginRunningResult::ErrResponse { status: 403, .. }),
            "got {res:?}"
        );
    }

    /// Origin not in the allowlist → dispatch returns a host error the
    /// guest observes (here: it answers 503 itself).
    #[tokio::test]
    async fn callout_denied_by_allowlist() {
        let f = write_temp_wasm(GUEST_CALLOUT);
        let config = callout_config(f.path(), vec!["https://allowed.example.com".to_string()], false);
        let plugin = WasmPlugin::create_request(&config).unwrap();
        let mut session = callout_mock("http://127.0.0.1:9999");
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(
            matches!(res, PluginRunningResult::ErrResponse { status: 503, .. }),
            "got {res:?}"
        );
    }

    /// Guest pauses on a callout but never resumes → host fail-mode
    /// fallback: failClose returns 500, failOpen continues.
    #[tokio::test]
    async fn callout_no_resume_falls_back_to_fail_mode() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let f = write_temp_wasm(GUEST_CALLOUT_NO_RESUME);
        for (fail_open, expect_500) in [(false, true), (true, false)] {
            let config = callout_config(f.path(), vec![server.uri()], fail_open);
            let plugin = WasmPlugin::create_request(&config).unwrap();
            let mut session = callout_mock(&server.uri());
            let mut log = PluginLog::new("Wasm");
            let res = plugin.run_request(&mut session, &mut log).await;
            if expect_500 {
                assert!(
                    matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
                    "failClose must 500, got {res:?}"
                );
            } else {
                assert_eq!(res, PluginRunningResult::GoodNext, "failOpen must continue");
            }
        }
    }

    /// Body-observation guest: `proxy_on_http_response_body` reads the
    /// current chunk window via `get_buffer_bytes(HttpResponseBody)` and
    /// returns the number of bytes it saw (a test-only contract that lets
    /// the host assert the window was visible).
    const GUEST_BODY_OBSERVER: &str = r#"
    (module
      (import "env" "proxy_get_buffer_bytes"
        (func $get_buf (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_http_response_body") (param i32 i32 i32) (result i32)
        (drop (call $get_buf (i32.const 1) (i32.const 0) (i32.const 65536)
                             (i32.const 256) (i32.const 260)))
        (i32.load (i32.const 260)))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// Body-observation: the guest sees exactly the current chunk window.
    #[tokio::test]
    async fn body_observation_guest_sees_chunk_window() {
        use crate::plugins::http::wasm::vm_manager::WasmVmManager;
        let f = write_temp_wasm(GUEST_BODY_OBSERVER);
        let c = config_for(f.path(), false);
        let mgr = WasmVmManager::new_for_test();
        let entry = mgr.get_or_load(&c, &runtime_cache_key(&c)).await.expect("loads");
        let mut lease = entry.acquire(100).expect("lease");
        lease.state_mut().begin_phase(
            WasmPhase::ResponseBody,
            Vec::new(),
            Vec::new(),
            std::collections::HashMap::new(),
        );
        lease.state_mut().body_chunk = b"hello-chunk".to_vec();
        let seen = lease.call_on_response_body(11, false).expect("hook ok");
        assert_eq!(seen, 11, "guest must observe exactly the staged chunk bytes");

        // Next chunk replaces the window — no accumulation.
        lease.state_mut().begin_phase(
            WasmPhase::ResponseBody,
            Vec::new(),
            Vec::new(),
            std::collections::HashMap::new(),
        );
        lease.state_mut().body_chunk = b"x".to_vec();
        let seen = lease.call_on_response_body(1, true).expect("hook ok");
        assert_eq!(seen, 1, "window must be the current chunk only");
    }

    /// Guest whose body hook traps — the stream must keep flowing.
    const GUEST_BODY_TRAP: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_http_response_body") (param i32 i32 i32) (result i32) unreachable)
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// Plugin-level body filter: never throttles, never panics — including
    /// with guests that have no body hook and one whose body hook traps.
    #[tokio::test]
    async fn body_filter_is_nonintrusive() {
        for wat in [GUEST_BODY_OBSERVER, GUEST_SET_HEADER, GUEST_TRAP, GUEST_BODY_TRAP] {
            let f = write_temp_wasm(wat);
            let c = config_for(f.path(), false);
            // Warm the cache so try_get hits.
            global_vm_manager()
                .get_or_load(&c, &runtime_cache_key(&c))
                .await
                .expect("loads");
            let plugin = WasmPlugin::create_body_filter(&c).unwrap();
            let mut session = MockPluginSession::new();
            let chunk = Some(bytes::Bytes::from_static(b"data"));
            let r = plugin.run_upstream_response_body_filter(&chunk, false, &mut session);
            assert!(r.is_none(), "body observation must never throttle");
            let r = plugin.run_upstream_response_body_filter(&None, true, &mut session);
            assert!(r.is_none());
        }
    }

    /// First real proxy-wasm-SDK guest through the host: the reference
    /// `header-demo` module built by `edgion-tests/wasm/build.sh`. Verifies
    /// the request-phase contract (`x-wasm-added: yes`) documented in
    /// `edgion-tests/wasm/README.md`.
    #[tokio::test]
    async fn reference_guest_module_loads_and_runs() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../edgion-tests/wasm/artifacts/header_demo.wasm"
        );
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: reference artifact not built");
            return;
        }
        let mut c = WasmConfig::default();
        c.source.file = Some(path.to_string());
        let plugin = WasmPlugin::create_request(&c).unwrap();
        let mut session = base_mock();
        session
            .expect_set_request_header()
            .withf(|k, v| k == "x-wasm-added" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
    }

    /// Response-phase coverage for the same reference guest: verifies the
    /// `x-wasm-resp: yes` contract. The sync `run_upstream_response_filter`
    /// hook uses `try_get`, which only hits once the module is warmed in the
    /// cache — drive one request-phase call first to populate it.
    ///
    /// The module cache is global (process-wide) and keyed by the source
    /// file path, so this test copies the artifact into its own temp file
    /// rather than reusing the shared path constant: that keeps its cache
    /// key disjoint from `reference_guest_module_loads_and_runs` (and any
    /// concurrently-running preload from that test's `WasmPlugin::build`),
    /// avoiding a transient `try_lock` race on the shared cache slot.
    #[tokio::test]
    async fn reference_guest_response_phase_sets_header() {
        let src_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../edgion-tests/wasm/artifacts/header_demo.wasm"
        );
        if !std::path::Path::new(src_path).exists() {
            eprintln!("skipping: reference artifact not built");
            return;
        }
        let bytes = std::fs::read(src_path).expect("read reference artifact");
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), &bytes).unwrap();

        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());

        // Warm the module cache so the sync response-phase `try_get` hits.
        global_vm_manager()
            .get_or_load(&c, &runtime_cache_key(&c))
            .await
            .expect("module loads");

        let plugin = WasmPlugin::create_response_filter(&c).unwrap();
        let mut session = base_mock();
        session.expect_response_headers().returning(Vec::new);
        session.expect_get_response_status().return_const(Some(200u16));
        session
            .expect_set_response_header()
            .withf(|k, v| k == "x-wasm-resp" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_upstream_response_filter(&mut session, &mut log);
        assert_eq!(res, PluginRunningResult::GoodNext);
    }
}
