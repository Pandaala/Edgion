//! Wasm plugin — executes proxy-wasm guests at the request-header and
//! response-header phases. See mod.rs docs for scope and limitations.

use std::collections::HashMap;

use async_trait::async_trait;

use super::host_abi::{HeaderOp, LocalResponse, WasmPhase, MAP_HTTP_REQUEST_HEADERS, MAP_HTTP_RESPONSE_HEADERS};
use super::vm_manager::{global_vm_manager, VmLease};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::WasmConfig;

const PLUGIN_NAME: &str = "Wasm";

pub struct WasmPlugin {
    name: String,
    config: WasmConfig,
    cache_key: String,
}

impl WasmPlugin {
    fn build(config: &WasmConfig) -> Self {
        let plugin = WasmPlugin {
            name: PLUGIN_NAME.to_string(),
            config: config.clone(),
            cache_key: config.source.cache_key(),
        };
        // Warm the module cache so the first request does not pay compile
        // latency (best-effort; lazy load covers the miss).
        global_vm_manager().spawn_preload(config);
        plugin
    }

    pub fn create_request(config: &WasmConfig) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self::build(config)))
    }

    pub fn create_response_filter(config: &WasmConfig) -> Option<Box<dyn UpstreamResponseFilter>> {
        Some(Box::new(Self::build(config)))
    }

    fn fail_mode(&self, log: &mut PluginLog, reason: &str) -> PluginRunningResult {
        if self.config.fail_open {
            log.push_fmt("wasm {}:fail-open", la![reason]);
            PluginRunningResult::GoodNext
        } else {
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

    /// Common per-phase execution once a lease is held. Returns the plugin
    /// result after draining ops / local response onto the session.
    fn run_leased(
        &self,
        mut lease: VmLease,
        phase: WasmPhase,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let request_headers = session.request_headers();
        let response_headers = if phase == WasmPhase::ResponseHeaders {
            session.response_headers()
        } else {
            Vec::new()
        };
        let num_headers = match phase {
            WasmPhase::RequestHeaders => request_headers.len() as i32,
            WasmPhase::ResponseHeaders => response_headers.len() as i32,
        };
        let props = self.properties(session, phase);
        lease
            .state_mut()
            .begin_phase(phase, request_headers, response_headers, props);

        let call = match phase {
            WasmPhase::RequestHeaders => lease.call_on_request_headers(num_headers),
            WasmPhase::ResponseHeaders => lease.call_on_response_headers(num_headers),
        };
        let action = match call {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, "wasm guest trap");
                return self.fail_mode(log, "trap");
            }
        };

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
        if action != 0 {
            // Pause without a local response cannot be honored in v1.
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
        let entry = match global_vm_manager().get_or_load(&self.config).await {
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
        self.run_leased(lease, WasmPhase::RequestHeaders, session, log)
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
            global_vm_manager().spawn_preload(&self.config);
            return self.fail_mode(log, "not loaded");
        };
        let lease = match entry.acquire(self.config.timeout_ms) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "wasm instance acquire failed");
                return self.fail_mode(log, "acquire");
            }
        };
        self.run_leased(lease, WasmPhase::ResponseHeaders, session, log)
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
        global_vm_manager().get_or_load(&c).await.expect("module loads");

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
