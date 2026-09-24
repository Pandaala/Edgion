//! Wasm plugin — executes proxy-wasm guests at the request-header,
//! request-body (buffer mode selected by an exported hook, with optional
//! operator `body:` policy), and
//! response-header phases. See mod.rs docs for scope and limitations.

use bytes::Bytes;
use edgion_resources::response_header_policy::is_protected_response_header;
use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use std::sync::Arc;

use super::callout::resolve_callouts_on;
use super::host_abi::{HeaderOp, LocalResponse, WasmPhase, MAP_HTTP_REQUEST_HEADERS, MAP_HTTP_RESPONSE_HEADERS};
use super::vm_manager::{global_vm_manager, CalloutGrant, ModuleEntry, VmLease, WasmRuntime};
use crate::observe::metrics::registry::{record_wasm_body_pause_unsupported, record_wasm_fail_mode};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{
    DyeOutcome, PluginLog, PluginSession, RequestFilter, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::WasmConfig;
use edgion_resources::EdgionErrorCode;

const PLUGIN_NAME: &str = "Wasm";

pub struct WasmPlugin {
    name: String,
    /// Mutable Wasm state belongs to this plugin instance. The common plugin
    /// runtime keeps this owner alive while requests still use the plugin.
    runtime: WasmRuntime,
    /// Bounded-cardinality source descriptor for metrics.
    metrics_label: String,
    /// Normalized callout allowlist origins, shared into each phase's
    /// `HostState` via `arm_dispatch`. Empty = callouts disabled.
    callout_allowlist: Arc<[String]>,
}

impl WasmPlugin {
    fn build(config: &WasmConfig) -> Self {
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
        // Keep the normalized authorization set stable and duplicate-free.
        callout_allowlist.sort();
        callout_allowlist.dedup();
        let callout_allowlist: Arc<[String]> = Arc::from(callout_allowlist);
        let runtime = global_vm_manager().new_runtime(
            config,
            CalloutGrant {
                allowlist: callout_allowlist.clone(),
                timeout_ms: config.callout_timeout_ms,
            },
        );
        let plugin = WasmPlugin {
            name: PLUGIN_NAME.to_string(),
            runtime,
            metrics_label: config.source.metrics_label(),
            callout_allowlist,
        };
        // Warm this runtime so the first request does not pay compile
        // latency (best-effort; lazy load covers the miss).
        plugin.runtime.spawn_preload();
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
        if self.runtime.config().fail_open {
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
                body: Some(Bytes::from_static(b"wasm plugin error")),
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
        p.insert("source.address".to_string(), session.remote_ip().to_string());
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

        let Ok(action) = lease.call_on_response_headers(num_headers) else {
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            return self.fail_mode(session, log, "trap");
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
            .arm_dispatch(self.callout_allowlist.clone(), self.runtime.config().callout_timeout_ms);

        let Ok((ctx_id, action)) = lease.call_on_request_headers_open(num_headers) else {
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            return self.fail_mode(session, log, "trap");
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
    /// `proxy_on_request_body`. `read_request_body` returns bytes already
    /// captured by an optional pre-execution body gate or lazily captures them
    /// while the request is still in the request-body phase. A mock or alternate
    /// adapter may return `Err(Unimplemented)`, which routes through `fail_mode`
    /// like any other body-read failure. A `NotBuffered` refusal is distinct: it
    /// fails closed unconditionally regardless of `failOpen`, since the guest's
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
            // ctx as an `EdgionErrorCode`, recorded by the read adapter.
            Err(BodyReadError::NotBuffered) => {
                log.push_err("wasm body-refused");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: Some(Bytes::from_static(b"body unavailable")),
                };
            }
            Err(BodyReadError::BufferLimitExceeded { .. }) => {
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
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
                session.record_error(EdgionErrorCode::BodyCaptureStorageFailed);
                log.push_err("wasm body-storage-failed");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            // Genuine read failures keep the configured policy, but must still
            // be alertable: `fail_mode` only writes a `PluginLog` line (nested
            // per-plugin array), while the access log's structured diagnostics
            // array is populated solely by `ctx.record_error`. Mirror the
            // `body_gate.rs::body_read_failure_result` mapping so both paths
            // agree.
            Err(err) => {
                session.record_error(match err {
                    BodyReadError::TooLarge { .. } => EdgionErrorCode::BodyReadTooLarge,
                    BodyReadError::BufferLimitExceeded { .. } => EdgionErrorCode::BodyReadTooLarge,
                    BodyReadError::Timeout => EdgionErrorCode::BodyReadTimeout,
                    BodyReadError::Incomplete => EdgionErrorCode::BodyReadAborted,
                    BodyReadError::Unimplemented => EdgionErrorCode::BodyReadUnimplemented,
                    // Handled above; unreachable here.
                    BodyReadError::NotBuffered => EdgionErrorCode::BodyRefusedStage,
                    BodyReadError::StorageFailure => EdgionErrorCode::BodyCaptureStorageFailed,
                    BodyReadError::GenerationConflict => EdgionErrorCode::BodyMutationConflict,
                });
                return self.fail_mode(session, log, "body read");
            }
        };
        let Ok(mut lease) = entry.acquire(self.runtime.config().timeout_ms) else {
            return self.fail_mode(session, log, "acquire");
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
        let Ok(action) = lease.call_on_request_body(size, true) else {
            return self.fail_mode(session, log, "trap");
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
                session.record_error(match error {
                    BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                        EdgionErrorCode::BodyReadTooLarge
                    }
                    BodyReadError::StorageFailure => EdgionErrorCode::BodyCaptureStorageFailed,
                    BodyReadError::GenerationConflict => EdgionErrorCode::BodyMutationConflict,
                    BodyReadError::Timeout => EdgionErrorCode::BodyReadTimeout,
                    BodyReadError::Incomplete => EdgionErrorCode::BodyReadAborted,
                    BodyReadError::Unimplemented => EdgionErrorCode::BodyReadUnimplemented,
                    BodyReadError::NotBuffered => EdgionErrorCode::BodyRefusedStage,
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
        let outcome = resolve_callouts_on(
            lease,
            ctx_id,
            self.runtime.config().callout_timeout_ms,
            &self.metrics_label,
        )
        .await?;
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
    ///   impl's `run_request`). The compiled guest exporting
    ///   `proxy_on_request_body` selects lazy capture (probed once at load via
    ///   `ModuleEntry::has_request_body_hook()`). `Wasm` has no INTRINSIC
    ///   `body_requirement()` — `EdgionPlugin::body_requirement()` returns
    ///   `None` for every `Wasm` config, because the guest's hooks are only
    ///   known after module load, not from the plugin's own config — but it
    ///   DOES `accepts_body()`, so an optional `body:` block on a Wasm entry is accepted at
    ///   preparse (unlike a plugin with no `body_requirement()` and no
    ///   `accepts_body()`, which would be rejected). `resolve_body_requirement`
    ///   (`plugin_runtime.rs`) passes that operator policy through as-is and
    ///   wraps this plugin's `RequestFilter` in a `BodyGatedRequestFilter`. The gate
    ///   buffers the body (capped by the resolved gateway-wide physical limit) BEFORE
    ///   the inner Wasm filter runs; `run_request_body_phase` then re-awaits
    ///   the same shared facade (`PluginSession::read_request_body`), which
    ///   must return the already-buffered bytes rather than re-reading the
    ///   stream. On success, wasm drives `proxy_on_request_body` **once** with
    ///   the full body (`body_size` = full length, `end_of_stream = true`,
    ///   `get_buffer_bytes(HttpRequestBody)` returns the whole thing), and the
    ///   guest can deny via `proxy_send_local_response` or atomically publish
    ///   a staged request-body rewrite — this is the only body path that can
    ///   deny or rewrite the request. Without an operator `body:` block,
    ///   `run_request_body_phase` starts lazy capture only after the headers
    ///   hook returns a clean Continue. A `NotBuffered` refusal (the request is
    ///   past the request-body stage) is always fail-close, unconditionally —
    ///   `failOpen` governs wasm *runtime* failures, not admission. A genuine
    ///   read failure — including `Unimplemented` from a mock or alternate
    ///   adapter — goes through `fail_mode`, so it honors `failOpen`, whose
    ///   default is fail-close (`fail_open` defaults to `false`). Either way the
    ///   failure is recorded via `ctx.record_error` so it is alertable in the
    ///   access log, not just logged to `PluginLog`.
    /// - **Response streaming observation** (this method, unchanged). `body_size`
    ///   is the CURRENT chunk's length and `get_buffer_bytes(HttpResponseBody)`
    ///   returns only that chunk. It cannot buffer — the `UpstreamResponseBodyFilter`
    ///   trait hands us `&Option<Bytes>` (immutable) and returns `Option<Duration>`,
    ///   so there is no way to withhold a chunk or accumulate one, no way to deny,
    ///   and no way to see the whole body (headers are already sent by the time a
    ///   response body guest runs).
    ///
    /// The mode a plugin gets is decided statically — by which hooks the
    /// compiled guest module exports and its placement in the operator's
    /// plugin array (request stage vs. response body stage). An optional
    /// request-side `body:` block adds pre-execution policy; it does not select
    /// capture. The mode is not chosen by a per-chunk
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
        let Some(entry) = self.runtime.try_get_or_schedule_refresh() else {
            self.runtime.spawn_preload();
            return None;
        };
        // Header-only guests pay nothing per chunk: skip the lease entirely
        // when the module has no body hook (probed once at load).
        if !entry.has_body_hook() {
            return None;
        }
        let chunk_budget_ms = (self.runtime.config().timeout_ms / 10).max(10);
        let Ok(mut lease) = entry.acquire(chunk_budget_ms) else {
            // Per-chunk hot path: acquire failure is non-fatal (stream continues);
            // no tracing here (Iron rule 3).
            return None;
        };
        let state = lease.state_mut();
        // Lean phase begin: body observation needs no header snapshot or
        // property map (per-chunk cost discipline).
        state.begin_phase(WasmPhase::ResponseBody, Vec::new(), Vec::new(), HashMap::new());
        state.body_chunk = body.clone().unwrap_or_default();
        let size = state.body_chunk.len() as i32;
        let action = lease.call_on_response_body(size, end_of_stream);
        // A healthy lease returns to the module pool when this function ends.
        // Drop the chunk reference first so an idle VM never pins response
        // payload storage between callbacks.
        lease.state_mut().body_chunk = Bytes::new();
        match action {
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
                // Replace editable fields. Transport-owned response fields survive;
                // omitted ordinary fields (credentials, cookies, identity) do not.
                for name in remove {
                    if map == MAP_HTTP_RESPONSE_HEADERS && is_protected_response_header(&name) {
                        continue;
                    }
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
                    if map == MAP_HTTP_RESPONSE_HEADERS && is_protected_response_header(&name) {
                        continue;
                    }
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
    let body = (!local.body.is_empty()).then(|| Bytes::from(local.body));
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
        let Ok(entry) = self.runtime.get_or_load().await else {
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            return self.fail_mode(session, log, "load");
        };
        let Ok(lease) = entry.acquire(self.runtime.config().timeout_ms) else {
            return self.fail_mode(session, log, "acquire");
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
        let Some(entry) = self.runtime.try_get_or_schedule_refresh() else {
            self.runtime.spawn_preload();
            return self.fail_mode(session, log, "not loaded");
        };
        let Ok(lease) = entry.acquire(self.runtime.config().timeout_ms) else {
            // Hot path: `fail_mode` records the metric and PluginLog entry; no tracing.
            return self.fail_mode(session, log, "acquire");
        };
        self.run_leased_response(lease, session, log)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/plugin_test.rs"]
mod tests;
