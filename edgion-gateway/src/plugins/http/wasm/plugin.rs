//! Wasm plugin — executes proxy-wasm guests at the request-header,
//! request-body (buffer mode, gated by an operator `body:` block), and
//! response-header phases. See mod.rs docs for scope and limitations.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use std::sync::Arc;

use super::callout::resolve_callouts_on;
use super::host_abi::{HeaderOp, LocalResponse, WasmPhase, MAP_HTTP_REQUEST_HEADERS, MAP_HTTP_RESPONSE_HEADERS};
use super::vm_manager::{global_vm_manager, runtime_cache_key, CalloutGrant, ModuleEntry, VmLease};
use crate::observe::metrics::registry::{record_wasm_body_pause_unsupported, record_wasm_fail_mode};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{
    DyeOutcome, PluginLog, PluginSession, RequestFilter, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::WasmConfig;
use edgion_resources::EdgionStatus;

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
        let mut callout_allowlist: Vec<String> = config
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
        // Canonical order so grants from plugins listing the same origins in
        // a different order compare equal in the tick-callout registry.
        callout_allowlist.sort();
        callout_allowlist.dedup();
        let plugin = WasmPlugin {
            name: PLUGIN_NAME.to_string(),
            config: config.clone(),
            cache_key: cache_key.clone(),
            metrics_label: config.source.metrics_label(),
            callout_allowlist: Arc::from(callout_allowlist),
        };
        // Pin the cache entry for this plugin's lifetime (released in Drop;
        // the manager evicts the entry after a grace period once no plugin
        // references it anymore). The grant feeds the module-level
        // tick-callout decision (see `TickCalloutState`).
        global_vm_manager().register_plugin(&plugin.cache_key, plugin.callout_grant());
        // Warm the module cache so the first request does not pay compile
        // latency (best-effort; lazy load covers the miss).
        global_vm_manager().spawn_preload(config, cache_key);
        plugin
    }

    /// This plugin's callout configuration as registered with (and removed
    /// from) the manager's tick-callout registry.
    fn callout_grant(&self) -> CalloutGrant {
        CalloutGrant {
            allowlist: self.callout_allowlist.clone(),
            timeout_ms: self.config.callout_timeout_ms,
        }
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

    /// Single choke point for every Wasm fail-open/fail-close decision
    /// (VM trap, callout failure, no-resume, module-load failure, pool-
    /// acquire failure). Also the single choke point for the dye suite's
    /// dependency-failure recording: `record_outcome` is called here, before
    /// returning `PluginRunningResult`, so it is always ordered before the
    /// runtime's own response write. `reason` is not further sub-classified
    /// (e.g. VM trap vs. epoch-deadline trap are not distinguished at this
    /// call site), so the kebab-cased `reason` itself is the most accurate
    /// cause available.
    fn fail_mode(&self, session: &mut dyn PluginSession, log: &mut PluginLog, reason: &str) -> PluginRunningResult {
        let cause = reason.replace(' ', "-");
        if self.config.fail_open {
            record_wasm_fail_mode(&self.metrics_label, reason, "open");
            log.push_fmt("wasm {}:fail-open", la![reason]);
            session.record_outcome(DyeOutcome::FailOpen, &cause);
            PluginRunningResult::GoodNext
        } else {
            record_wasm_fail_mode(&self.metrics_label, reason, "close");
            log.push_fmt("wasm {}:fail-close", la![reason]);
            session.record_outcome(DyeOutcome::FailClose, &cause);
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
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            Err(_) => return self.fail_mode(session, log, "trap"),
        };
        self.finish_phase(lease, session, log, action, false)
    }

    /// Request-headers execution with callout + Pause support. The context
    /// stays open across callout delivery so the paused guest can receive
    /// `proxy_on_http_call_response` and then resume (continue_request) or
    /// short-circuit (send_local_response).
    async fn run_leased_request(
        &self,
        entry: Arc<ModuleEntry>,
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
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            Err(_) => return self.fail_mode(session, log, "trap"),
        };

        let paused = action != 0;
        let had_callouts = !lease.state().pending_callouts.is_empty();
        if paused && had_callouts {
            if self.resolve_callouts(&mut lease, ctx_id, log).await.is_err() {
                // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
                return self.fail_mode(session, log, "callout");
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
            return self.fail_mode(session, log, "trap");
        }

        // A paused guest that neither continued nor answered locally after
        // all callouts completed gets the fail-mode treatment; a pause
        // without any callout keeps the v1 "pause-ignored" behavior.
        let state = lease.state();
        if paused && had_callouts && !state.continue_requested && state.local_response.is_none() {
            drop(lease);
            return self.fail_mode(session, log, "no-resume");
        }
        // The headers-phase result is produced exactly as before (the lease is
        // returned to the pool inside `finish_phase`). Only a clean GoodNext
        // continues into the request-body phase; any deny / local response /
        // fail-mode short-circuits and never buffers the body.
        let headers_result = self.finish_phase(lease, session, log, action, had_callouts);
        if !matches!(headers_result, PluginRunningResult::GoodNext) {
            return headers_result;
        }
        self.run_request_body_phase(&entry, session, log).await
    }

    /// Buffer-mode request-body drive. Runs only when the guest exports
    /// `proxy_on_request_body` and the body gate has buffered the body (the
    /// `read_request_body` await returns `Ok`). A binary without the body
    /// capability returns `Err(Unimplemented)`, which routes through
    /// `fail_mode` like any other body-read failure. A `NotBuffered` refusal is distinct: it fails
    /// closed unconditionally regardless of `failOpen`, since the guest's
    /// body hook would otherwise inspect nothing (see the read match below).
    /// A fresh lease is re-acquired from `entry` (the same
    /// already-loaded `ModuleEntry` the headers phase used, passed in so no
    /// racy `try_get` re-lookup is needed): the headers phase already
    /// returned its lease to the pool via `finish_phase`. A guest deny (local
    /// response) is drained the same way as the other phases.
    async fn run_request_body_phase(
        &self,
        entry: &Arc<ModuleEntry>,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !entry.has_request_body_hook() {
            return PluginRunningResult::GoodNext;
        }
        let cap = session.ctx().request_body_config.max_body_size_bytes;
        let body = match session.read_request_body(Some(cap)).await {
            Ok(b) => b,
            // A refusal is unconditionally fail-close (design R5): the guest's
            // body hook would inspect nothing, and `failOpen` governs wasm
            // runtime failures, not admission. The precise reason is already on
            // ctx as an `EdgionStatus`, recorded by the read adapter.
            Err(BodyReadError::NotBuffered) => {
                log.push_err("wasm body-refused");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: Some("body unavailable".to_string()),
                };
            }
            Err(BodyReadError::BufferLimitExceeded { .. }) => {
                session.add_error(EdgionStatus::BodyReadTooLarge);
                log.push_err("wasm body-buffer-limit");
                return PluginRunningResult::ErrResponse {
                    status: 413,
                    body: None,
                };
            }
            // A capture-storage failure is unconditionally fail-close, matching
            // `body_gate.rs::body_read_failure_result`: consumed bytes were
            // never stored, so no forwarding copy of the body exists and
            // `failOpen` cannot help this request.
            Err(BodyReadError::StorageFailure) => {
                session.add_error(EdgionStatus::BodyCaptureStorageFailed);
                log.push_err("wasm body-storage-failed");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            // Genuine read failures keep the configured policy, but must still
            // be alertable: `fail_mode` only writes a `PluginLog` line (nested
            // per-plugin array), while the access log's structured `errors`
            // array is populated solely by `ctx.add_error`. Mirror the
            // `body_gate.rs::body_read_failure_result` mapping so both paths
            // agree.
            Err(err) => {
                session.add_error(match err {
                    BodyReadError::TooLarge { .. } => EdgionStatus::BodyReadTooLarge,
                    BodyReadError::BufferLimitExceeded { .. } => EdgionStatus::BodyReadTooLarge,
                    BodyReadError::Timeout => EdgionStatus::BodyReadTimeout,
                    BodyReadError::Incomplete => EdgionStatus::BodyReadAborted,
                    BodyReadError::Unimplemented => EdgionStatus::BodyReadUnimplemented,
                    // Handled above; unreachable here.
                    BodyReadError::NotBuffered => EdgionStatus::BodyRefusedStage,
                    BodyReadError::StorageFailure => EdgionStatus::BodyCaptureStorageFailed,
                });
                return self.fail_mode(session, log, "body read");
            }
        };
        let mut lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            Err(_) => return self.fail_mode(session, log, "acquire"),
        };
        let size = body.len() as i32;
        let request_headers = session.request_headers();
        let props = self.properties(session, WasmPhase::RequestBody);
        lease
            .state_mut()
            .begin_phase(WasmPhase::RequestBody, request_headers, Vec::new(), props);
        {
            let state = lease.state_mut();
            state.request_body = body.to_vec();
            state.request_body_limit = usize::try_from(cap).unwrap_or(usize::MAX);
        }
        let action = match lease.call_on_request_body(size, true) {
            Ok(a) => a,
            Err(_) => return self.fail_mode(session, log, "trap"),
        };
        if lease.state().local_response.is_some() {
            // The proxied request has ended. Finish the deliberate local
            // response without attempting an irrelevant BodyStorage write.
            return self.finish_phase(lease, session, log, action, false);
        }
        let request_body = {
            let state = lease.state_mut();
            state
                .request_body_modified
                .then(|| bytes::Bytes::from(std::mem::take(&mut state.request_body)))
        };
        if let Some(body) = request_body {
            if let Err(error) = session.overwrite_request_body(body).await {
                session.add_error(match error {
                    BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                        EdgionStatus::BodyReadTooLarge
                    }
                    BodyReadError::StorageFailure => EdgionStatus::BodyCaptureStorageFailed,
                    BodyReadError::Timeout => EdgionStatus::BodyReadTimeout,
                    BodyReadError::Incomplete => EdgionStatus::BodyReadAborted,
                    BodyReadError::Unimplemented => EdgionStatus::BodyReadUnimplemented,
                    BodyReadError::NotBuffered => EdgionStatus::BodyRefusedStage,
                });
                return self.fail_mode(session, log, "body mutation");
            }
        }
        // Apply request-header operations only after the Body generation is
        // committed. On fail-open publication failure, neither half of a
        // guest's logically coupled Content-Type/body rewrite leaks through.
        self.finish_phase(lease, session, log, action, false)
    }

    /// Execute queued callouts and deliver each result to the paused guest
    /// (see [`resolve_callouts_on`] for the bounded delivery loop), recording
    /// the delivered/leftover counts in the plugin log. Leftovers after the
    /// round cap are dropped (guest is dispatching in a loop); the no-resume
    /// fallback in the caller decides the request's fate.
    async fn resolve_callouts(&self, lease: &mut VmLease, ctx_id: i32, log: &mut PluginLog) -> Result<(), String> {
        let outcome = resolve_callouts_on(lease, ctx_id, self.config.callout_timeout_ms, &self.metrics_label).await?;
        log.push_fmt(
            "wasm callouts={} leftover={}",
            la![
                outcome.delivered.to_string().as_str(),
                outcome.leftover.to_string().as_str()
            ],
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
    /// `proxy_on_response_body` + `get_buffer_bytes(HttpResponseBody)`.
    ///
    /// ## TWO-MODE MODEL — read before changing
    ///
    /// Edgion selects the wasm body mode **statically**, per plugin instance,
    /// instead of letting the guest choose dynamically via
    /// `Action::Pause`/`Continue` the way Envoy does:
    ///
    /// - **Request buffer mode** (see `run_request_body_phase` above —
    ///   a different method on this same type, invoked from the `RequestFilter`
    ///   impl's `run_request`). Selection requires BOTH an operator `body:`
    ///   block on the Wasm entry AND the compiled guest module exporting
    ///   `proxy_on_request_body` (probed once at load via
    ///   `ModuleEntry::has_request_body_hook()`). `Wasm` has no INTRINSIC
    ///   `body_requirement()` — `EdgionPlugin::body_requirement()` returns
    ///   `None` for every `Wasm` config, because the guest's hooks are only
    ///   known after module load, not from the plugin's own config — but it
    ///   DOES `accepts_body()`, so `body:` on a Wasm entry IS accepted at
    ///   preparse (unlike a plugin with no `body_requirement()` and no
    ///   `accepts_body()`, which would be rejected). `resolve_body_requirement`
    ///   (`plugin_runtime.rs`) treats that operator `body:` as the SOLE source
    ///   of truth for Wasm and passes it through as-is, which wraps this
    ///   plugin's `RequestFilter` in a `BodyGatedRequestFilter`. The gate
    ///   buffers the body (capped by the resolved gateway-wide physical limit) BEFORE
    ///   the inner Wasm filter runs; `run_request_body_phase` then re-awaits
    ///   the same shared facade (`PluginSession::read_request_body`), which
    ///   must return the already-buffered bytes rather than re-reading the
    ///   stream. On success, wasm drives `proxy_on_request_body` **once** with
    ///   the full body (`body_size` = full length, `end_of_stream = true`,
    ///   `get_buffer_bytes(HttpRequestBody)` returns the whole thing), and the
    ///   guest can deny via `proxy_send_local_response` or atomically publish
    ///   a staged request-body rewrite — this is the only body path that can
    ///   deny or rewrite the request. Without an operator `body:` block the
    ///   gate is never installed and `run_request_body_phase` still calls
    ///   `read_request_body` ungated (see that method's doc for why). A
    ///   `NotBuffered` refusal (the gate was never installed / the request is
    ///   past the request-body stage) is always fail-close, unconditionally —
    ///   `failOpen` governs wasm *runtime* failures, not admission. A genuine
    ///   read failure — including `Unimplemented` from a binary built without
    ///   the request-buffering capability — goes through `fail_mode`, so it honors
    ///   `failOpen`, whose default is fail-close (`fail_open` defaults to
    ///   `false`). Either way the failure is recorded via `ctx.add_error` so it
    ///   is alertable in the access log, not just logged to `PluginLog`. An
    ///   ungated read must still fail rather than auto-buffer — see the facade contract in
    ///   `docs/superpowers/specs/2026-07-22-wasm-body-wiring-design.md`.
    /// - **Response streaming observation** (this method, unchanged). `body_size`
    ///   is the CURRENT chunk's length and `get_buffer_bytes(HttpResponseBody)`
    ///   returns only that chunk. It cannot buffer — the `UpstreamResponseBodyFilter`
    ///   trait hands us `&Option<Bytes>` (immutable) and returns `Option<Duration>`,
    ///   so there is no way to withhold a chunk or accumulate one, no way to deny,
    ///   and no way to see the whole body (headers are already sent by the time a
    ///   response body guest runs).
    ///
    /// The mode a plugin gets is decided statically — by the operator `body:`
    /// block (request side only) combined with which hooks the compiled guest
    /// module exports, and by its placement in the operator's plugin array
    /// (request stage vs. response body stage) — not by a per-chunk
    /// `Continue`/`Pause` decision from the guest at runtime; this is a deliberate deviation from the
    /// proxy-wasm spec, which models buffering as dynamic (Envoy maps `Pause`
    /// onto `StopIterationAndBuffer` and lets its filter manager own the
    /// buffer).
    ///
    /// Consequence for THIS method: a guest written to the standard dynamic idiom
    /// (`if !end_of_stream { return Action::Pause }` then read the "whole body")
    /// would silently observe only the LAST chunk if run here — a false negative
    /// for a WAF or secret scanner. So a Pause on the response path is NOT
    /// silently dropped: it is counted via the bounded
    /// `edgion_wasm_body_pause_unsupported_total` metric, a reliable signal that
    /// the guest wants buffering it isn't getting. That buffering is available
    /// today on the *request* side for a guest exporting `proxy_on_request_body`
    /// (see above, once the shared facade is backed); on the *response* side it
    /// requires a response buffer mode this host does not yet
    /// build (the response-header-phase plugin would need to be registered on a
    /// gated async trait so the whole response body can be staged before the
    /// per-chunk stream starts — future work, see
    /// `tasks/wasm-followups/body-buffering-spec-semantics.md`). (This is a
    /// per-chunk data-plane hot path, so it must not trace — Iron rule 3; the
    /// metric is the only channel here.)
    ///
    /// Other constraints of this stage:
    /// - the body cannot be modified and the response cannot be short-circuited
    ///   (headers are already sent) — a guest verdict can only be logged
    ///   (`proxy_log`) or aggregated via shared data;
    /// - a trap never interrupts the stream (regardless of `failOpen`): the
    ///   lease is discarded and streaming continues.
    ///
    /// Each chunk runs under a reduced CPU budget (`timeoutMs / 10`) so a
    /// slow guest cannot stall a long stream chunk by chunk.
    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        _session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        let Some(entry) = global_vm_manager().try_get_or_schedule_refresh(&self.config, &self.cache_key) else {
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
            // Per-chunk hot path: acquire failure is non-fatal (stream continues);
            // no tracing here (Iron rule 3).
            Err(_) => return None,
        };
        let state = lease.state_mut();
        // Lean phase begin: body observation needs no header snapshot or
        // property map (per-chunk cost discipline).
        state.begin_phase(WasmPhase::ResponseBody, Vec::new(), Vec::new(), HashMap::new());
        state.body_chunk = body.as_ref().map(|b| b.to_vec()).unwrap_or_default();
        let size = state.body_chunk.len() as i32;
        match lease.call_on_response_body(size, end_of_stream) {
            Ok(action) if action != 0 => {
                // Pause: the guest expects the spec's cumulative-buffer
                // semantics, which this host does not implement (the guest then
                // silently observes only the current chunk). This is a data-plane
                // per-chunk hot path (Iron rule 3) and the body filter has no
                // PluginLog channel, so the incompatibility is surfaced only via
                // the bounded metric — never tracing.
                record_wasm_body_pause_unsupported(entry.metrics_label());
            }
            Ok(_) => {}
            // Body-hook trap: the stream keeps flowing; no tracing (Iron rule 3).
            Err(_) => {}
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
        global_vm_manager().unregister_plugin(&self.cache_key, &self.metrics_label, &self.callout_grant());
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
            HeaderOp::ReplaceAll { map, remove, pairs } => {
                // Whole-map replacement: first drop every name that existed
                // before the replace so omitted headers (credentials, cookies,
                // spoofable identity headers) do not survive as an additive set.
                for name in remove {
                    let _ = match map {
                        MAP_HTTP_REQUEST_HEADERS => session.remove_request_header(&name),
                        MAP_HTTP_RESPONSE_HEADERS => session.remove_response_header(&name),
                        _ => Ok(()),
                    };
                }
                // Install the new vector: `set` (replace-all) for the first
                // occurrence of each name, `append` for subsequent duplicates so
                // repeated values (e.g. multiple Set-Cookie) stay distinct
                // instead of collapsing to the last one.
                let mut seen: HashSet<String> = HashSet::new();
                for (name, value) in pairs {
                    let first = seen.insert(name.to_ascii_lowercase());
                    let _ = match (map, first) {
                        (MAP_HTTP_REQUEST_HEADERS, true) => session.set_request_header(&name, &value),
                        (MAP_HTTP_REQUEST_HEADERS, false) => session.append_request_header(&name, &value),
                        (MAP_HTTP_RESPONSE_HEADERS, true) => session.set_response_header(&name, &value),
                        (MAP_HTTP_RESPONSE_HEADERS, false) => session.append_response_header(&name, &value),
                        _ => Ok(()),
                    };
                }
                Ok(())
            }
            _ => Ok(()), // unknown map id: ignore
        };
        // Header-apply failure on the request hot path: ignored (best-effort op
        // drain); no tracing (Iron rule 3).
        let _ = result;
    }
}

fn finish_local_response(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    local: LocalResponse,
) -> PluginRunningResult {
    // The guest header map may repeat a name (e.g. multiple Set-Cookie).
    // Apply the first occurrence of each name with set semantics (replace any
    // carried-over value) and every later occurrence with append semantics, so
    // non-combinable fields like Set-Cookie survive instead of collapsing to
    // the last value. Set-semantics-for-all would drop every value but the last.
    let mut seen: HashSet<String> = HashSet::new();
    for (name, value) in &local.headers {
        let _ = if seen.insert(name.to_ascii_lowercase()) {
            session.set_response_header(name, value)
        } else {
            session.append_response_header(name, value)
        };
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
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            Err(_) => return self.fail_mode(session, log, "load"),
        };
        let lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            Err(_) => return self.fail_mode(session, log, "acquire"),
        };
        self.run_leased_request(entry, lease, session, log).await
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
        let Some(entry) = global_vm_manager().try_get_or_schedule_refresh(&self.config, &self.cache_key) else {
            global_vm_manager().spawn_preload(&self.config, self.cache_key.clone());
            return self.fail_mode(session, log, "not loaded");
        };
        let lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            Err(_) => return self.fail_mode(session, log, "acquire"),
        };
        self.run_leased_response(lease, session, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::PluginLog;
    use edgion_resources::plugin_configs::WasmRawConfig;

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

    /// The Rust SDK declares some standard ABI imports that Edgion does not
    /// implement. They are load-compatible for SDK portability, but invoking
    /// one must trap and enter the ordinary guest-trap failure policy.
    const GUEST_KNOWN_UNSUPPORTED_TRAP: &str = r#"
    (module
      (import "env" "proxy_close_stream" (func $close (param i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (call $close (i32.const 0)))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
      (func (export "proxy_abi_version_0_2_0"))
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
        // Default request-body-phase stubs: a WAT guest without a
        // `proxy_on_request_body` export never reaches these, but the real
        // proxy-wasm SDK guest does. Ungated (no `body:`) → `read_request_body`
        // returns `Unimplemented`, which now routes through `fail_mode` (Task
        // 7) rather than being skipped -- most callers of `base_mock` never
        // hit this arm at all since their WAT guests lack the export. Tests
        // that need an actual buffered body use `body_mock` instead.
        s.expect_ctx().return_const(crate::EdgionHttpContext::new());
        s.expect_read_request_body()
            .returning(|_| Err(crate::plugins::runtime::traits::session::BodyReadError::Unimplemented));
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
        // Task 9: the VM-trap fail-close arm records `FailClose` with the
        // kebab-cased reason ("trap") as cause.
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[tokio::test]
    async fn trap_fail_open_continues() {
        let f = write_temp_wasm(GUEST_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).unwrap();
        let mut session = base_mock();
        // Task 9: the VM-trap fail-open arm records `FailOpen` with the same cause.
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "trap")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn known_unsupported_sdk_import_traps_through_failure_policy() {
        let f = write_temp_wasm(GUEST_KNOWN_UNSUPPORTED_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).unwrap();
        let mut session = base_mock();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[tokio::test]
    async fn gateway_survives_trap_then_serves_next_request() {
        let f = write_temp_wasm(GUEST_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).unwrap();
        for _ in 0..3 {
            let mut session = base_mock();
            session.expect_record_outcome().return_const(());
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
        // Task 9: the module-load-failure fail-close arm records `FailClose`
        // with cause "load".
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "load")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    /// A WASM guest local response that repeats a header name (two Set-Cookie
    /// pairs) must not collapse to the last value. Reproduces the request-stage
    /// session adapter (no real ResponseHeader yet → ops are queued) and drains
    /// the queue onto a real ResponseHeader, asserting both cookies survive.
    #[test]
    fn local_response_preserves_duplicate_set_cookie() {
        use crate::ctx::PendingHeaderOp;
        use parking_lot::Mutex;
        use pingora_http::ResponseHeader;
        use std::sync::Arc;

        let queue: Arc<Mutex<Vec<PendingHeaderOp>>> = Arc::new(Mutex::new(Vec::new()));
        let mut session = MockPluginSession::new();
        let q_set = queue.clone();
        session.expect_set_response_header().returning(move |name, value| {
            q_set.lock().push(PendingHeaderOp::Set {
                name: name.to_string(),
                value: value.to_string(),
            });
            Ok(())
        });
        let q_app = queue.clone();
        session.expect_append_response_header().returning(move |name, value| {
            q_app.lock().push(PendingHeaderOp::Append {
                name: name.to_string(),
                value: value.to_string(),
            });
            Ok(())
        });

        // Mixed case on the repeated name exercises the case-insensitive dedup.
        let local = LocalResponse {
            status: 302,
            body: Vec::new(),
            headers: vec![
                ("Set-Cookie".to_string(), "session=abc".to_string()),
                ("set-cookie".to_string(), "refresh=xyz".to_string()),
                ("location".to_string(), "/login".to_string()),
            ],
        };
        let mut log = PluginLog::new("Wasm");
        let res = finish_local_response(&mut session, &mut log, local);
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 302, .. }));

        // Drain the queued ops onto a real ResponseHeader (the path a
        // request-stage local reply actually takes) and assert both survived.
        let mut resp = ResponseHeader::build(302, None).unwrap();
        for op in queue.lock().drain(..) {
            op.apply(&mut resp);
        }
        let cookies: Vec<String> = resp
            .headers
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies, vec!["session=abc".to_string(), "refresh=xyz".to_string()]);
        // A single-valued header is unaffected by the set-then-append change.
        assert_eq!(resp.headers.get("location").unwrap().to_str().unwrap(), "/login");
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
            // Task 9: the no-resume fail-mode fallback records the matching
            // outcome with cause "no-resume".
            let expected_outcome = if fail_open {
                DyeOutcome::FailOpen
            } else {
                DyeOutcome::FailClose
            };
            session
                .expect_record_outcome()
                .withf(move |outcome, value| *outcome == expected_outcome && value == "no-resume")
                .times(1)
                .return_const(());
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

    /// Body-observation guest: `proxy_on_response_body` reads the
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
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
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

    /// Guest whose body hook returns Action::Pause (1) — i.e. it is written to
    /// the standard proxy-wasm cumulative-buffer idiom this host does not
    /// implement.
    const GUEST_BODY_PAUSE: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    /// A guest that Pauses on the body hook expects cumulative buffering, which
    /// this host cannot provide. The stream must keep flowing (never throttle,
    /// never break); the incompatibility is surfaced only via the bounded
    /// `edgion_wasm_body_pause_unsupported_total` metric (no tracing on the
    /// per-chunk hot path — Iron rule 3).
    #[tokio::test]
    async fn body_pause_is_metered_and_never_breaks_the_stream() {
        use crate::plugins::http::wasm::vm_manager::WasmVmManager;
        let f = write_temp_wasm(GUEST_BODY_PAUSE);
        let c = config_for(f.path(), false);

        // The Pausing guest exports a body hook, so the body-filter stage will
        // lease and run it (rather than skipping header-only guests).
        let mgr = WasmVmManager::new_for_test();
        let entry = mgr.get_or_load(&c, &runtime_cache_key(&c)).await.expect("loads");
        assert!(entry.has_body_hook(), "guest exports proxy_on_response_body");

        // Plugin path: a Pausing guest must not throttle or break the stream.
        global_vm_manager()
            .get_or_load(&c, &runtime_cache_key(&c))
            .await
            .expect("module loads");
        let plugin = WasmPlugin::create_body_filter(&c).unwrap();
        let mut session = MockPluginSession::new();
        let chunk = Some(bytes::Bytes::from_static(b"abc"));
        for eos in [false, true] {
            assert!(
                plugin
                    .run_upstream_response_body_filter(&chunk, eos, &mut session)
                    .is_none(),
                "a Pausing body guest must never throttle or interrupt the stream"
            );
        }
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
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32) unreachable)
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
        // This test is about the header phase, not body-read policy; the SDK
        // guest also exports `proxy_on_request_body`, and `base_mock`'s
        // `Unimplemented` stub now runs through `fail_mode` (Task 7), so
        // `fail_open` must be set to keep that phase a no-op here.
        c.fail_open = true;
        let plugin = WasmPlugin::create_request(&c).unwrap();
        let mut session = base_mock();
        session
            .expect_set_request_header()
            .withf(|k, v| k == "x-wasm-added" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_record_outcome().return_const(());
        // The SDK guest's request-body hook runs `read_request_body` ->
        // `Unimplemented` -> `fail_mode` (Task 7), which now also records the
        // matching `EdgionStatus` (R17) before `fail_open` softens the result.
        session.expect_add_error().return_const(());
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

    // ── Reference SDK guest: new-capability coverage ─────────────────────────
    //
    // These drive the REAL rebuilt `header_demo.wasm` (proxy-wasm Rust SDK)
    // through the host for callout / tick+shared-data / body observation — the
    // same guest the integration suite uses, so they catch SDK-vs-host ABI
    // drift the WAT fixtures cannot. Each test copies the artifact to its own
    // temp file so its cache entry (and thus its SharedData + tick task) is
    // disjoint from the others'.

    const REF_ARTIFACT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-tests/wasm/artifacts/header_demo.wasm"
    );

    /// Copy the committed reference artifact to a fresh temp file (disjoint
    /// cache key). Returns None when the artifact has not been built.
    fn ref_artifact_temp() -> Option<tempfile::NamedTempFile> {
        if !std::path::Path::new(REF_ARTIFACT).exists() {
            eprintln!("skipping: reference artifact not built");
            return None;
        }
        let bytes = std::fs::read(REF_ARTIFACT).expect("read reference artifact");
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), &bytes).unwrap();
        Some(f)
    }

    fn ref_mock(headers: Vec<(&str, &str)>) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        let owned: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        s.expect_request_headers().returning(move || owned.clone());
        let hmap: std::collections::HashMap<String, String> =
            headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        s.expect_header_value().returning(move |name| hmap.get(name).cloned());
        s.expect_get_path().return_const("/x".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_remote_addr().return_const("1.2.3.4".to_string());
        // A guest deny path (send_http_response) applies local-response headers.
        s.expect_set_response_header().returning(|_, _| Ok(()));
        // The proxy-wasm Rust SDK exports `proxy_on_request_body` on every
        // guest, so the request-body phase now runs for the reference module.
        // These tests exercise the UNGATED plugin (no `body:` requirement),
        // where `read_request_body` returns `Unimplemented`, which now routes
        // through `fail_mode` (Task 7) rather than being skipped. Every test
        // function here that actually reaches this arm sets `c.fail_open =
        // true` (with its own comment) so the phase stays a no-op GoodNext.
        s.expect_ctx().return_const(crate::EdgionHttpContext::new());
        s.expect_read_request_body()
            .returning(|_| Err(crate::plugins::runtime::traits::session::BodyReadError::Unimplemented));
        s
    }

    /// Real SDK guest: callout to an auth endpoint that returns 200 for a valid
    /// bearer token → guest resumes and applies `x-wasm-added`. Also asserts the
    /// forwarded Authorization header reached the callout target.
    #[tokio::test]
    async fn reference_guest_callout_authorized_resumes() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let Some(f) = ref_artifact_temp() else { return };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/verify"))
            .and(header("authorization", "Bearer valid-token"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());
        c.callout_allowlist = vec![server.uri()];
        // Not testing body-read policy here; the SDK guest also exports
        // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
        // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
        c.fail_open = true;

        let plugin = WasmPlugin::create_request(&c).unwrap();
        let mut session = ref_mock(vec![
            ("host", "t.example.com"),
            ("x-wasm-callout-url", &server.uri()),
            ("authorization", "Bearer valid-token"),
        ]);
        session
            .expect_set_request_header()
            .withf(|k, v| k == "x-wasm-added" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_record_outcome().return_const(());
        // See `reference_guest_module_loads_and_runs` for why this fires (R17).
        session.expect_add_error().return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext, "authorized callout must resume");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// Real SDK guest: callout returns 401 for a bad token → guest denies with
    /// a 403 local response.
    #[tokio::test]
    async fn reference_guest_callout_unauthorized_denies() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let Some(f) = ref_artifact_temp() else { return };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());
        c.callout_allowlist = vec![server.uri()];

        let plugin = WasmPlugin::create_request(&c).unwrap();
        let mut session = ref_mock(vec![
            ("host", "t.example.com"),
            ("x-wasm-callout-url", &server.uri()),
            ("authorization", "Bearer nope"),
        ]);
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(
            matches!(res, PluginRunningResult::ErrResponse { status: 403, .. }),
            "unauthorized callout must deny, got {res:?}"
        );
    }

    /// Real SDK guest: `on_tick` increments a shared counter; a later request
    /// reads it into `x-wasm-ticks`. Proves the background tick task runs and
    /// shared data is visible to request hooks. (Guest tick period is 1s.)
    #[tokio::test]
    async fn reference_guest_tick_increments_shared_counter() {
        let Some(f) = ref_artifact_temp() else { return };
        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());
        // Not testing body-read policy here; the SDK guest also exports
        // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
        // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
        c.fail_open = true;

        // Load warms one instance, which runs on_configure (sets the 1s tick)
        // and arms the module tick task.
        global_vm_manager()
            .get_or_load(&c, &runtime_cache_key(&c))
            .await
            .expect("module loads");
        // Wait for at least two ticks.
        tokio::time::sleep(std::time::Duration::from_millis(2300)).await;

        let plugin = WasmPlugin::create_request(&c).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let seen2 = seen.clone();
        let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-ticks", "1")]);
        session
            .expect_set_request_header()
            .withf(move |k, v| {
                if k == "x-wasm-ticks" {
                    seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                }
                true
            })
            .returning(|_, _| Ok(()));
        session.expect_record_outcome().return_const(());
        // See `reference_guest_module_loads_and_runs` for why this fires (R17).
        session.expect_add_error().return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
        assert!(
            seen.load(std::sync::atomic::Ordering::Relaxed) >= 1,
            "on_tick must have fired at least once and be readable via shared data"
        );
    }

    /// Real SDK guest: with `pluginConfig` set to a callout origin, `on_tick`
    /// dispatches a GET /verify callout; the 200 callback (delivered on the
    /// root context by the tick task) bumps the shared `calloutticks`
    /// counter, which a later request reads into `x-wasm-calloutticks`.
    /// End-to-end proof of the tick-callout loop with the reference guest.
    #[tokio::test]
    async fn reference_guest_tick_callout_polls_and_publishes() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let Some(f) = ref_artifact_temp() else { return };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/verify"))
            .and(header("authorization", "Bearer valid-token"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());
        c.plugin_config = Some(WasmRawConfig::string(server.uri()));
        c.callout_allowlist = vec![server.uri()];
        // Not testing body-read policy here; the SDK guest also exports
        // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
        // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
        c.fail_open = true;

        // Building the plugin registers its callout grant and preloads the
        // module; the eager warm instance runs on_configure (1s tick).
        let plugin = WasmPlugin::create_request(&c).unwrap();

        // Poll until a tick's callout has been published, rather than sleeping a fixed 2.5s
        // and asserting once. The tick is 1s and each one does an HTTP callout round-trip, so
        // a fixed wait is a bet on wall-clock scheduling: under a full `cargo test --all` the
        // machine is saturated and 2.5s can pass without two ticks landing, which failed this
        // assertion for a reason that has nothing to do with what it is testing. Polling to a
        // generous deadline keeps the test fast on an idle machine and honest on a busy one.
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut res = None;
        while std::time::Instant::now() < deadline {
            let seen2 = seen.clone();
            let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-callout", "1")]);
            session
                .expect_set_request_header()
                .withf(move |k, v| {
                    if k == "x-wasm-calloutticks" {
                        seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                    }
                    true
                })
                .returning(|_, _| Ok(()));
            session.expect_record_outcome().return_const(());
            // See `reference_guest_module_loads_and_runs` for why this fires (R17).
            session.expect_add_error().return_const(());
            let mut log = PluginLog::new("Wasm");
            res = Some(plugin.run_request(&mut session, &mut log).await);
            if seen.load(std::sync::atomic::Ordering::Relaxed) >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        assert_eq!(res, Some(PluginRunningResult::GoodNext));
        assert!(
            seen.load(std::sync::atomic::Ordering::Relaxed) >= 1,
            "tick callouts must have completed and been published via shared data"
        );
        assert!(
            !server.received_requests().await.unwrap().is_empty(),
            "the tick callout must reach the target"
        );
    }

    /// Real SDK guest: the read-only body hook adds each chunk's byte count to
    /// a shared counter; a later request reads it into `x-wasm-bodybytes`.
    /// Ties body observation → shared data → cross-request read.
    #[tokio::test]
    async fn reference_guest_body_observation_accumulates_bytes() {
        let Some(f) = ref_artifact_temp() else { return };
        let mut c = WasmConfig::default();
        c.source.file = Some(f.path().to_string_lossy().into_owned());
        // Not testing request body-read policy here; the SDK guest also
        // exports `proxy_on_request_body`, whose `Unimplemented` stub now runs
        // through `fail_mode` (Task 7), so `fail_open` keeps that phase a
        // no-op for the request below (this test's own body path goes through
        // the response body filter, not `read_request_body`).
        c.fail_open = true;
        global_vm_manager()
            .get_or_load(&c, &runtime_cache_key(&c))
            .await
            .expect("module loads");

        // Drive two response-body chunks through the body filter.
        let body_plugin = WasmPlugin::create_body_filter(&c).unwrap();
        let mut bsession = MockPluginSession::new();
        let chunk = Some(bytes::Bytes::from_static(&[b'x'; 100]));
        assert!(body_plugin
            .run_upstream_response_body_filter(&chunk, false, &mut bsession)
            .is_none());
        assert!(body_plugin
            .run_upstream_response_body_filter(&chunk, true, &mut bsession)
            .is_none());

        // A request reads the accumulated byte count.
        let req_plugin = WasmPlugin::create_request(&c).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let seen2 = seen.clone();
        let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-body", "1")]);
        session
            .expect_set_request_header()
            .withf(move |k, v| {
                if k == "x-wasm-bodybytes" {
                    seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                }
                true
            })
            .returning(|_, _| Ok(()));
        session.expect_record_outcome().return_const(());
        // See `reference_guest_module_loads_and_runs` for why this fires (R17).
        session.expect_add_error().return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = req_plugin.run_request(&mut session, &mut log).await;
        assert_eq!(res, PluginRunningResult::GoodNext);
        assert!(
            seen.load(std::sync::atomic::Ordering::Relaxed) >= 200,
            "body hook must have counted both 100-byte chunks into shared data"
        );
    }

    /// Request-body buffer-mode guest: `proxy_on_request_body` sends a 403
    /// local response when the buffered body is non-empty (a WAF/scanner deny),
    /// otherwise continues. Also exports a no-op `proxy_on_request_headers` so
    /// the request-headers phase runs to GoodNext and the body phase is reached.
    const GUEST_REQUEST_BODY_DENY: &str = r#"
    (module
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (if (i32.gt_s (local.get 1) (i32.const 0))
          (then (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0)))))
        (i32.const 0)))
    "#;

    const GUEST_REQUEST_BODY_REWRITE: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_replace_header_map_value"
        (func $set_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (data (i32.const 16) "x-body-rewritten")
      (data (i32.const 40) "yes")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        (drop (call $set_hdr (i32.const 0) (i32.const 16) (i32.const 16)
                             (i32.const 40) (i32.const 3)))
        (i32.const 0)))
    "#;

    const GUEST_REQUEST_BODY_REWRITE_THEN_DENY: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                          (i32.const 0) (i32.const 0) (i32.const 0)
                          (i32.const 0) (i32.const 0)))
        (i32.const 0)))
    "#;

    const GUEST_REQUEST_BODY_REWRITE_THEN_TRAP: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        unreachable))
    "#;

    /// Request-phase mock with a ctx (for the body-size cap resolution) and a
    /// single scripted `read_request_body` outcome. Built standalone rather
    /// than on `base_mock` so this is the only `read_request_body` expectation
    /// (mockall matches FIFO; `base_mock`'s unlimited stub would otherwise win).
    fn body_mock(read: crate::plugins::runtime::traits::session::BodyReadResult) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_request_headers()
            .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
        s.expect_get_path().return_const("/x".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_remote_addr().return_const("1.2.3.4".to_string());
        s.expect_ctx().return_const(crate::EdgionHttpContext::new());
        s.expect_read_request_body().return_once(move |_| read);
        s
    }

    /// A guest with a request-body hook that denies on a non-empty buffered
    /// body must short-circuit with the guest's local response (403) once the
    /// body gate has buffered the body (`read_request_body` -> Ok).
    #[tokio::test]
    async fn request_body_guest_receives_full_body_and_can_deny() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"scan-me")));
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            res,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn request_body_guest_commits_rewrite_to_shared_body() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == b"aXYZdef")
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_set_request_header()
            .withf(|name, value| name == "x-body-rewritten" && value == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("Wasm");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn request_body_local_response_skips_irrelevant_rewrite_publication() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE_THEN_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
        let mut log = PluginLog::new("Wasm");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None,
            },
            "a BodyStorage write must not replace a deliberate local denial"
        );
    }

    #[tokio::test]
    async fn request_body_trap_discards_staged_rewrite() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE_THEN_TRAP);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
        session.expect_overwrite_request_body().times(0);
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");

        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
    }

    #[tokio::test]
    async fn request_body_publication_failure_is_fail_closed_and_observable() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == b"aXYZdef")
            .times(1)
            .returning(|_| Err(BodyReadError::StorageFailure));
        session.expect_set_request_header().times(0);
        session
            .expect_add_error()
            .withf(|status| *status == edgion_resources::EdgionStatus::BodyCaptureStorageFailed)
            .times(1)
            .return_const(());
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "body-mutation")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");

        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
    }

    #[tokio::test]
    async fn request_body_publication_fail_open_leaks_no_partial_header_commit() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
        session
            .expect_overwrite_request_body()
            .times(1)
            .returning(|_| Err(BodyReadError::StorageFailure));
        session.expect_set_request_header().times(0);
        session
            .expect_add_error()
            .withf(|status| *status == edgion_resources::EdgionStatus::BodyCaptureStorageFailed)
            .times(1)
            .return_const(());
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "body-mutation")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    /// When `read_request_body` returns `Unimplemented` (ungated plugin / no
    /// buffering backing), the deny guest's body hook is not driven — but this
    /// is a genuine read failure, not admission, so it goes through
    /// `fail_mode` like any other Wasm runtime failure (Task 7). With the
    /// `fail_open: false` default that means fail-closed (500), not a silent
    /// GoodNext skip; `wasm_read_failure_honors_fail_open` pins the
    /// `fail_open: true` override of this same read error.
    #[tokio::test]
    async fn request_body_read_failure_fails_closed_by_default() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Err(BodyReadError::Unimplemented));
        session
            .expect_add_error()
            .withf(|status| *status == edgion_resources::EdgionStatus::BodyReadUnimplemented)
            .times(1)
            .return_const(());
        session.expect_record_outcome().return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    /// R17: an ungated wasm module (no operator `body:` block) whose body read
    /// fails with `Unimplemented` must still record an `EdgionStatus` via
    /// `ctx.add_error`, not just a `PluginLog` line — otherwise the newly-common
    /// default-500 for a body-hook wasm guest is invisible in the access log's
    /// structured `errors` array. Pins the mapping used at the
    /// `run_request_body_phase` call site (mirrors `body_gate.rs`'s
    /// `body_read_failure_result`).
    #[tokio::test]
    async fn ungated_body_read_unimplemented_records_edgion_status() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Err(BodyReadError::Unimplemented));
        session
            .expect_add_error()
            .withf(|status| *status == edgion_resources::EdgionStatus::BodyReadUnimplemented)
            .times(1)
            .return_const(());
        session.expect_record_outcome().return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(
            matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
            "fail_open defaults to false, so the ungated read failure must still fail-close"
        );
    }

    /// R5: `failOpen: true` governs wasm RUNTIME failures, not admission. A refusal
    /// means the guest's body hook would inspect nothing, so it must fail closed
    /// even though this plugin is configured fail-open.
    #[tokio::test]
    async fn wasm_refusal_is_fail_close_even_with_fail_open_configured() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).expect("plugin");
        let mut session = body_mock(Err(BodyReadError::NotBuffered));
        let mut log = PluginLog::new("Wasm");

        let res = plugin.run_request(&mut session, &mut log).await;
        assert!(
            matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
            "a refusal must not be softened by failOpen, got {res:?}"
        );
    }

    /// R6: a genuine read failure keeps the existing knob. With `failOpen: true`
    /// an `Unimplemented` read leaves the body hook undriven and the request
    /// continues — the fail-close default for this same read error is pinned by
    /// `request_body_read_failure_fails_closed_by_default`.
    #[tokio::test]
    async fn wasm_read_failure_honors_fail_open() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), true)).expect("plugin");
        let mut session = body_mock(Err(BodyReadError::Unimplemented));
        session
            .expect_add_error()
            .withf(|status| *status == edgion_resources::EdgionStatus::BodyReadUnimplemented)
            .times(1)
            .return_const(());
        session.expect_record_outcome().return_const(());
        let mut log = PluginLog::new("Wasm");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    /// Request-body buffer-mode guest that reads the `host` request header
    /// (via `proxy_get_header_map_value` against `MAP_HTTP_REQUEST_HEADERS`)
    /// from inside `proxy_on_request_body`, and denies with 403 only when
    /// BOTH the header lookup succeeds (`STATUS_OK` == 0) AND the buffered
    /// body is non-empty. This proves the body phase snapshot carries the
    /// real request headers (regression guest for the "empty headers in
    /// request-body phase" bug — before the fix, `get_hdr` would return
    /// `STATUS_NOT_FOUND` and the guest would never deny even with a body).
    const GUEST_REQUEST_BODY_READS_HEADER: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "host")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (if (i32.and
              (i32.eqz (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 4)
                                      (i32.const 256) (i32.const 260)))
              (i32.gt_s (local.get 1) (i32.const 0)))
          (then (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0)))))
        (i32.const 0)))
    "#;

    /// Regression for the request-body phase snapshotting empty request
    /// headers: a guest that reads the `host` header AND the body inside
    /// `proxy_on_request_body` must see both — the header lookup succeeds and
    /// the buffered body is non-empty, so the guest denies with 403. Before
    /// the fix, `begin_phase(WasmPhase::RequestBody, ...)` was seeded with an
    /// empty header vector, so `proxy_get_header_map_value` would return
    /// `STATUS_NOT_FOUND` and this guest would never deny.
    #[tokio::test]
    async fn request_body_guest_can_read_request_header_and_body() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_READS_HEADER);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let mut session = body_mock(Ok(bytes::Bytes::from_static(b"scan-me")));
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            res,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            },
            "guest must see the request header AND the body in the body phase"
        );
    }

    /// `BodyGatedRequestFilter` (the operator `body:` gate) buffers the
    /// request body once to enforce its own read-failure/size policy, then
    /// hands off to the wrapped `WasmPlugin`, whose own request-body phase
    /// re-reads the SAME session via `read_request_body` a second time (see
    /// `run_request_body_phase`). `PluginSession::read_request_body` must be
    /// idempotent (cached bytes served on every call), so this mock uses
    /// `.returning(...)` rather than `.return_once(...)` to prove both the
    /// gate and the inner Wasm body phase can call it and both see the body.
    #[tokio::test]
    async fn body_gate_and_wasm_body_phase_both_read_request_body() {
        let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
        let plugin = WasmPlugin::create_request(&config_for(f.path(), false)).expect("plugin");
        let gate = crate::plugins::runtime::BodyGatedRequestFilter::new(
            plugin,
            edgion_resources::resources::edgion_plugins::BodyRequirement::fail_close(),
        );

        let mut session = MockPluginSession::new();
        session
            .expect_request_headers()
            .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
        session.expect_get_path().return_const("/x".to_string());
        session.expect_get_method().return_const("GET".to_string());
        session.expect_header_value().returning(|_| None);
        session.expect_remote_addr().return_const("1.2.3.4".to_string());
        session.expect_ctx().return_const(crate::EdgionHttpContext::new());
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        session.expect_read_request_body().returning(move |_| {
            calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(bytes::Bytes::from_static(b"scan-me"))
        });

        let mut log = PluginLog::new("Wasm");
        let res = gate.run_request(&mut session, &mut log).await;
        // The guest hook (GUEST_REQUEST_BODY_DENY) receives the buffered body
        // in `proxy_on_request_body` and denies on the non-empty body.
        assert_eq!(
            res,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            }
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the gate and the wasm body phase must both read the request body"
        );
    }

    #[test]
    fn apply_ops_replace_all_removes_omitted_request_headers() {
        // ReplaceAll must be a true replacement: a name present before the
        // replace (authorization) but absent from the new vector must be
        // removed, not left to leak downstream as an additive set.
        let ops = vec![HeaderOp::ReplaceAll {
            map: MAP_HTTP_REQUEST_HEADERS,
            remove: vec!["authorization".to_string(), "x-keep".to_string()],
            pairs: vec![("x-keep".to_string(), "new".to_string())],
        }];
        let mut session = MockPluginSession::new();
        session
            .expect_remove_request_header()
            .withf(|n| n == "authorization")
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|n| n == "x-keep")
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_set_request_header()
            .withf(|n, v| n == "x-keep" && v == "new")
            .times(1)
            .returning(|_, _| Ok(()));
        apply_ops(&mut session, ops);
    }

    #[test]
    fn apply_ops_replace_all_preserves_duplicate_response_values() {
        // Repeated names in the replacement (e.g. multiple Set-Cookie) must
        // stay distinct: first occurrence set (replace-all), later occurrences
        // appended, instead of collapsing to the last value.
        let ops = vec![HeaderOp::ReplaceAll {
            map: MAP_HTTP_RESPONSE_HEADERS,
            remove: vec!["set-cookie".to_string()],
            pairs: vec![
                ("set-cookie".to_string(), "a=1".to_string()),
                ("set-cookie".to_string(), "b=2".to_string()),
            ],
        }];
        let mut session = MockPluginSession::new();
        session
            .expect_remove_response_header()
            .withf(|n| n == "set-cookie")
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_set_response_header()
            .withf(|n, v| n == "set-cookie" && v == "a=1")
            .times(1)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header()
            .withf(|n, v| n == "set-cookie" && v == "b=2")
            .times(1)
            .returning(|_, _| Ok(()));
        apply_ops(&mut session, ops);
    }
}
