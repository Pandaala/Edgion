#!/usr/bin/env python3
"""Check unit-test mounts in all production crates.

Cargo compilation and test-identity comparisons remain necessary for macro
expansions, cfg combinations, and privacy checks.
"""

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "edgion-common"
MIGRATED_CRATES = (
    "edgion-common",
    "edgion-dsl",
    "edgion-cli",
    "edgion-controller",
    "edgion-algo",
    "edgion-waf",
    "edgion-wasm",
    "edgion-resources",
    "edgion-gateway",
)
CFG = re.compile(r"#\s*\[\s*cfg\s*\(\s*(?:test\s*\)|all\s*\(\s*test\s*,[^)]*\)\s*\))\s*\]")
PATH = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]')
ATTRIBUTE = re.compile(r'#\s*\[[^\]]*\]')
INCLUDE = re.compile(r'include!\s*\(\s*"([^"]+)"\s*\)\s*;')
MODULE = re.compile(r"(?:pub\s*(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z_0-9]*)\s*([;{])")
TEST = re.compile(r"#\s*\[\s*(?:(?:tokio|async_std)\s*::\s*)?test\b")
USE = re.compile(r"(?:pub\s*(?:\([^)]*\))?\s+)?use\s+")
ASSOCIATED_ITEM = re.compile(
    r"(?:pub\s*(?:\([^)]*\))?\s+)?(?:(?:async|const)\s+)?(?:fn|const)\s+([A-Za-z_][A-Za-z_0-9]*)\b"
)

# Associated test helpers stay attached to their production impls because Rust
# does not accept include! in impl item position. Each owner/name is explicit.
GATEWAY_ASSOCIATED_TEST_HELPERS = {
    Path('ai/routing/adaptive.rs'): ('get_or_create',),
    Path('ai/conversion/chat_to_anthropic/response.rs'): ('new',),
    Path('ai/runtime/attempt_runtime.rs'): ('record_test_attempt', 'settlement_decisions_for_test'),
    Path('ai/runtime/circuit_breaker.rs'): ('in_flight',),
    Path('ai/runtime/credential_outlier.rs'): ('admit', 'apply_retry_after', 'configure_backend', 'current_handle', 'effective_handle_for_test', 'fallback_handle_for_test', 'force_expired_cooldown_for_test', 'record_failure', 'record_success', 'state'),
    Path('ai/quota/mod.rs'): ('observed_output_for_test',),
    Path('ai/accounting/usage.rs'): ('add_output_units_for_test',),
    Path('backends/discovery/coordinator.rs'): ('expire_outlier_deadlines_for_test', 'set_completion_barrier_for_test', 'set_degrade_threshold', 'set_next_epoch_for_test'),
    Path('backends/discovery/endpoint_slice/ep_slice_store.rs'): ('update_data_only',),
    Path('backends/discovery/services/service_store.rs'): ('update',),
    Path('backends/outlier_detection/state.rs'): ('expire_deadlines_for_test', 'num_ejections'),
    Path('backends/outlier_detection/store.rs'): ('has_runtime_identity',),
    Path('backends/policy/backend_tls/backend_tls_policy_store.rs'): ('effective_from_policy', 'from_policy', 'target_buckets'),
    Path('backends/policy/backend_traffic/retry_budget.rs'): ('notify',),
    Path('cli/config.rs'): ('admin_listen', 'probe_listen'),
    Path('conf_sync/cache_client/cache.rs'): ('reset',),
    Path('conf_sync/cache_client/cache_data.rs'): ('compressed_event_count_for_test', 'flush_compressed_events_for_test', 'set_conf_processor_for_test', 'set_sync_version'),
    Path('conf_sync/conf_client/config_client.rs'): ('client_cache_keys', 'edgion_backends', 'edgion_tls', 'publication_test_cache'),
    Path('ctx.rs'): ('dye_handled_request_names', 'has_grpc_info', 'has_plugin_aux', 'has_request_body_observers', 'has_response_processing', 'has_response_stream_processors', 'has_tls_info', 'owner', 'request_body_stream_handler_count', 'response_stream_processor_count', 'with_labels'),
    Path('geoip/provider.rs'): ('clear',),
    Path('lb/ewma/score.rs'): ('stored', 'stored_state'),
    Path('link_sys/providers/otlp/client.rs'): ('publish_lazy_test_pipeline',),
    Path('link_sys/providers/webhook/client.rs'): ('from_managed',),
    Path('link_sys/providers/webhook/runtime.rs'): ('notify',),
    Path('link_sys/runtime/runner.rs'): ('desired_link_for_test', 'spawn_with_published'),
    Path('link_sys/runtime/store.rs'): ('install',),
    Path('observe/logs/buffer.rs'): ('contains',),
    Path('plugins/http/common/bounded_regex_replacement.rs'): ('needs_captures',),
    Path('plugins/http/common/outbound/client.rs'): ('test_with_static_target',),
    Path('plugins/http/common/outbound/pinned_resolver.rs'): ('addrs_for_test',),
    Path('plugins/http/dsl/lang/builtins.rs'): ('ALL',),
    Path('plugins/http/ext_proc/attached.rs'): ('test_lazy', 'test_state'),
    Path('plugins/http/ext_proc/plugin.rs'): ('build',),
    Path('plugins/http/hmac_auth/plugin.rs'): ('check_body_digest', 'check_body_digest_hash', 'compute_hmac', 'credentials_ptr', 'verify_body_digest'),
    Path('plugins/http/openid_connect/jwks_provider.rs'): ('install_scripted_http', 'seed_discovery', 'seed_stale_jwks'),
    Path('plugins/http/openid_connect/plugin.rs'): ('new',),
    Path('plugins/http/request_mirror/budget.rs'): ('for_test',),
    Path('plugins/http/wasm/shared_data.rs'): ('total_bytes',),
    Path('plugins/http/wasm/vm_manager.rs'): ('engine', 'get_or_load_with_linker', 'new_for_test', 'new_with_engine_result', 'pool_len', 'shared_data_for_test'),
    Path('plugins/runtime/conditions/evaluator.rs'): ('evaluate', 'evaluate_detail', 'hmac_credentials_ptr'),
    Path('plugins/runtime/edgion_config_data/conf_handler.rs'): ('insert_raw_for_test',),
    Path('plugins/runtime/log.rs'): ('contains', 'render_for_test'),
    Path('plugins/stream/connection_rate_limit/local_gate.rs'): ('exhaust_generations_for_test',),
    Path('plugins/stream/tls_route/tls_route_plugin_runtime.rs'): ('from_test_plugins',),
    Path('request_body/state.rs'): ('cached_body_is_materialized',),
    Path('request_body/state/enabled.rs'): ('cached_body_is_materialized',),
    Path('request_body/storage.rs'): ('is_materialized', 'with_limits', 'with_memory_limit'),
    Path('request_trace_id/w3c.rs'): ('trace_id', 'traceparent'),
    Path('response/driver.rs'): ('ordinary_count',),
    Path('routes/grpc/match_engine.rs'): ('new', 'shares_hosts'),
    Path('routes/http/routes_mgr.rs'): ('get_global_routes',),
    Path('routes/tls/gateway_tls_routes.rs'): ('from_units', 'match_route'),
    Path('routes/udp/edgion_udp.rs'): ('session_count',),
    Path('runtime/matching/tls.rs'): ('for_test', 'refresh_session_context_for_test'),
    Path('runtime/store/listener_gateway_info.rs'): ('from_entries_for_test',),
    Path('sse/chain.rs'): ('coalesce_count', 'copied_bytes'),
    Path('sse/window.rs'): ('accounting', 'complete'),
    Path('tls/store/cert_matcher.rs'): ('match_attachment_sni', 'match_sni'),
    Path('tls/store/tls_store.rs'): ('contains', 'get_validation_status', 'new_isolated'),
}

# Source-local test instrumentation, fields, initializers, and branches cannot
# be moved without changing the production item they instrument.
GATEWAY_SOURCE_LOCAL = {
    # Observe canonical TLS audit calls without changing the production logger.
    Path('observe/logs/ssl_log.rs'): (
        'tests::record(entry);',
    ),
    Path('lb/compiled_selector.rs'): (
        'tests::record_selector_set_build();',
    ),
    Path('ai/runtime/attempt_runtime.rs'): (
        'self.settlement_decisions.push(operation);',
        'settlement_decisions: Vec<SettlementOperation>,',
    ),
    Path('backends/discovery/coordinator.rs'): (
        'completion_barrier: Mutex::new(None),',
        'completion_barrier: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,',
        'if let Some((reached, resume)) = self.completion_barrier.lock().clone() {',
    ),
    Path('backends/outlier_detection/store.rs'): (
        'percent_arbitrations: AtomicUsize,',
        'percent_arbitrations: AtomicUsize::new(0),',
        'svc.percent_arbitrations.fetch_add(1, Ordering::Relaxed);',
    ),
    Path('backends/policy/backend_traffic/retry_budget.rs'): (
        'clock: None,',
        'clock: Option<Arc<tests::TestClock>>,',
        'hook: None,',
        'hook: Option<Arc<dyn Fn(tests::Event) + Send + Sync>>,',
        'if let Some(clock) = &self.clock {',
        'self.notify(tests::Event::AfterCas);',
        'self.notify(tests::Event::AfterFirstLoad);',
        'self.notify(tests::Event::AfterSnapshot);',
        'self.notify(tests::Event::BeforeCas);',
    ),
    Path('backends/policy/backend_traffic/store.rs'): (
        'publish_counters::FULL_INDEX_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);',
        'publish_counters::PREPARE_POLICY_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);',
        'publish_counters::SIDE_EFFECT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);',
    ),
    Path('config/edgion_gateway/conf_handler_impl.rs'): (
        'let _cap = crate::link_sys::runtime::store::LINK_SYS_CAP_TEST_LOCK.lock();',
    ),
    Path('geoip/provider.rs'): (
        'if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Decode) {',
        'if LOOKUP_FAULT.with(|c| c.get()) == Some(LookupFault::Panic) {',
    ),
    Path('link_sys/providers/webhook/runtime.rs'): (
        'clock: None,',
        'clock: Option<std::sync::Arc<dyn Fn() -> u64 + Send + Sync>>,',
        'hook: None,',
        'hook: Option<std::sync::Arc<dyn Fn(tests::Event) + Send + Sync>>,',
        'if let Some(clock) = &self.clock {',
        'self.notify(tests::Event::AfterCas);',
        'self.notify(tests::Event::BeforeCas);',
    ),
    Path('link_sys/runtime/runner.rs'): (
        'if let Some(hook) = runner.publish_entry.lock().take() {',
        'publish_entry: Mutex::new(None),',
        'publish_entry: Mutex<Option<Box<dyn FnOnce() + Send>>>,',
    ),
    Path('plugins/http/conf_handler_impl.rs'): (
        'self.prepare_test_scope(data.keys().cloned().collect());',
        '{ let mut keys: HashSet<_> = self.publisher().snapshot().http_sources().keys().cloned().collect();',
    ),
    Path('plugins/http/ext_proc/client.rs'): (
        'InboundState::Test(receiver) => match receiver.recv().await {',
        'Test(tokio::sync::mpsc::Receiver<Result<ProcessingResponse, tonic::Status>>),',
        'if let InboundState::Test(receiver) = &mut self.inbound {',
    ),
    Path('plugins/http/openid_connect/jwks_provider.rs'): (
        'scripted_http: ScriptMutex::new(None),',
        'scripted_http: ScriptMutex<Option<Arc<ScriptedHttp>>>,',
        '{ let script = self.scripted_http.lock().clone();',
    ),
    # Per-instance session clock injection; no clock field or branch in production builds.
    Path('plugins/http/openid_connect/plugin.rs'): (
        'session_clock: Option<Arc<std::sync::atomic::AtomicU64>>,',
        'session_clock: None,',
    ),
    Path('plugins/http/openid_connect/session_cookie.rs'): (
        'if let Some(clock) = &self.session_clock {',
    ),
    Path('plugins/http/request_mirror/plugin.rs'): (
        'let send = arming_tests::consume_without_network(',
        'let writer_events = self',
        'writer_events: None,',
        'writer_events: Option<mpsc::UnboundedSender<arming_tests::WriterEvent>>,',
        '{ drop(_permit);',
    ),
    Path('plugins/http/wasm/vm_manager.rs'): (
        'active_tick_tasks: AtomicUsize,',
        'active_tick_tasks: AtomicUsize::new(0),',
        'let _test_task = TestTickTaskGuard::new(test_tick_state);',
        'let test_tick_state = self.tick_callouts.clone();',
    ),
    Path('plugins/stream/stream_plugin_store.rs'): (
        'self.prepare_test_scope(data.keys().cloned().collect());',
        '{ let mut keys: HashSet<_> = self.publisher().snapshot().stream_sources().keys().cloned().collect();',
    ),
    Path('request_body/storage.rs'): (
        'if let Some(hook) = test_hook {',
        'let test_hook = STORAGE_IO_TEST_HOOK.try_with(Arc::clone).ok();',
    ),
    Path('response/driver.rs'): (
        'let ordinary_count = body.ordinary.len();',
        'ordinary_count,',
        'ordinary_count: usize,',
    ),
    Path('runtime/store/listener_gateway_info.rs'): (
        'if let Some(index) = TEST_INDEX_OVERRIDES.with(|overrides| overrides.borrow().get(key).cloned()) {',
    ),
}

# These branches and fields must stay in their owning production items so the
# test configuration can exercise the same function or preserve struct shape.
SCOPED_EXCEPTIONS = {
    ("edgion-controller", Path("conf_mgr/conf_center/etcd/status.rs"), re.compile(r"read_barrier: Option<\(Arc<tokio::sync::Notify>, Arc<tokio::sync::Semaphore>\)>,")):
        "per-handler scheduling barrier for the retired Secret worker regression; no production field",
    ("edgion-controller", Path("conf_mgr/conf_center/etcd/status.rs"), re.compile(r"read_barrier: None,")):
        "test scheduling barrier initializer in the production constructor",
    ("edgion-controller", Path("conf_mgr/conf_center/etcd/status.rs"), re.compile(r"pub\(super\) fn with_read_barrier\(")):
        "associated helper installs the per-handler status-read scheduling barrier",
    ("edgion-controller", Path("conf_mgr/conf_center/etcd/status.rs"), re.compile(r"if let Some\(\(entered, release\)\) = &self\.read_barrier \{")):
        "pause at the real status-read await boundary without replacing the read result",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"force_unpruned_inspection: bool,")):
        "test-only switch for the unpruned inspection oracle",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"force_unpruned_inspection: self\.force_unpruned_inspection,")):
        "test-only oracle switch preserved by canonical TX clone policies",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::GroupScratchInitializations\b")):
        "test-only matcher cache first-use measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::BooleanScratchInitializations\b")):
        "test-only boolean cache first-use measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::CaptureScratchInitializations\b")):
        "test-only capture cache first-use measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::CaptureBooleanSearches\b")):
        "test-only original capture hit-search count at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::NameSelectorSearches\b")):
        "test-only raw-name selector search count at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"if context\.tx\.force_unpruned_inspection \{")):
        "test-only branch bypassing optional static activation proof",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::AvoidedPredicateGroups\b")):
        "test-only skipped static group measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::UnitTxClones\b")):
        "test-only accounting of new-unit TX clones after first-member matching",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"sequential_ascii: bool,")):
        "test-only switch for the sequential lexical oracle",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"sequential_ascii: false,")):
        "test-only sequential lexical oracle initializer",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"if self\.sequential_ascii \{")):
        "test-only branch into the source-relative sequential lexical oracle",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"fn evaluate_member_matches\(")):
        "test-only associated wrapper over the production reusable match buffer",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"force_json_cursor: bool,")):
        "test-only switch for the canonical JSON cursor oracle",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"force_json_cursor: false,")):
        "test-only canonical JSON cursor oracle initializer",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"if self\.force_json_cursor \{")):
        "test-only branch bypassing optional JSON summary replay",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"measurements::record\(Event::Completions\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"measurements::record\(Event::GrammarInstances\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"measurements::record\(Event::SourceBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_stream.rs"), re.compile(r"measurements::record\(Event::TokenCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_checker.rs"), re.compile(r"measurements::record\(Event::KeyCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_checker.rs"), re.compile(r"measurements::record\(Event::PathCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection.rs"), re.compile(r"measurements::record\(Event::Candidates\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection.rs"), re.compile(r"measurements::record\(Event::CaptureCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection.rs"), re.compile(r"measurements::record\(Event::DeferredCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection.rs"), re.compile(r"measurements::record\(Event::MatchedCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection.rs"), re.compile(r"measurements::record\(Event::NameCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::DetectorWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::DispatchGroups\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::DispatchWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::OperatorStages\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::OperatorWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::ProfileWalks\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::TransformSteps\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"measurements::record\(Event::TransformWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_runtime.rs"), re.compile(r"measurements::record\(Event::LexicalPeakBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_runtime.rs"), re.compile(r"measurements::record\(Event::LexicalWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_runtime.rs"), re.compile(r"measurements::record\(Event::StructurePeakBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_runtime.rs"), re.compile(r"measurements::record\(Event::StructureWork\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::FullScans\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::RangeQueries\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::RangeQueryFailures\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"measurements::record\(Event::TemporaryCaptureCopyBytes\b")):
        "test-only JSON mechanism measurement at the owning operation",
    ("edgion-waf", Path("seclang/json_inspection_collector.rs"), re.compile(r"let dispatch_work_before = self\.store\.usage\(\)\.work;")):
        "test-only snapshot of existing dispatch work accounting",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"scans:\s*usize\b")):
        "test-only successful cache scan counter field",
    ("edgion-waf", Path("seclang/transaction.rs"), re.compile(r"\{\s*cache\.scans \+= 1;")):
        "test-only successful cache scan counter update",
    ("edgion-controller", Path("conf_mgr/conf_center/common.rs"), re.compile(r"let crd_dir\b")):
        "test-only CRD path fallback inside parse_resource_content_offline",
    ("edgion-waf", Path("seclang/compiler.rs"), re.compile(r"if COMPILER_ALLOCATION_FAILURE\.with\b")):
        "allocation failure injection inside reserve_exact",
    ("edgion-waf", Path("seclang/semantic.rs"), re.compile(r"if VALIDATION_ALLOCATION_FAILURE\.with\b")):
        "allocation failure injection inside reserve_exact",
    ("edgion-waf", Path("seclang/semantic.rs"), re.compile(r"inspected_folds:\s*usize\b")):
        "test-only chain projection counter field",
    ("edgion-waf", Path("seclang/semantic.rs"), re.compile(r"inspected_folds:\s*0\b")):
        "test-only chain projection counter initializer",
    ("edgion-waf", Path("seclang/semantic.rs"), re.compile(r"\{\s*projection\.inspected_folds \+= 1;")):
        "test-only chain projection counter update",
}


def mask_non_code(text: str) -> str:
    """Keep offsets/newlines while removing Rust comments and string contents."""
    out = list(text)
    i = 0
    n = len(text)

    def blank(start: int, end: int) -> None:
        for k in range(start, end):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
        elif text.startswith("/*", i):
            depth = 1
            end = i + 2
            while end < n and depth:
                if text.startswith("/*", end):
                    depth += 1
                    end += 2
                elif text.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank(i, end)
            i = end
        else:
            char = re.match(r"(?:b)?'(?:\\.|[^'\\\n])'", text[i:])
            if char:
                blank(i, i + char.end())
                i += char.end()
                continue
            raw = re.match(r'(?:b|br|r)?(#+)?"', text[i:])
            if raw:
                hashes = raw.group(1) or ""
                end_marker = '"' + hashes
                start = i + raw.end()
                end = text.find(end_marker, start)
                end = n if end < 0 else end + len(end_marker)
                if not hashes and text[i] not in "r":
                    # Ordinary strings can contain escaped quotes.
                    end = start
                    while end < n:
                        if text[end] == "\\":
                            end += 2
                        elif text[end] == '"':
                            end += 1
                            break
                        else:
                            end += 1
                blank(i, end)
                i = end
            else:
                i += 1
    return "".join(out)


def check() -> list[str]:
    errors = []
    mounts = {}
    support = (CRATE / "tests" / "support" / "outbound_tls.rs").resolve()
    for source in sorted((CRATE / "src").rglob("*.rs")):
        original = source.read_text()
        code = mask_non_code(original)
        relative = source.relative_to(CRATE / "src")
        for match in CFG.finditer(code):
            cursor = match.end()
            while cursor < len(code) and code[cursor].isspace():
                cursor += 1
            path_match = None
            while attribute := ATTRIBUTE.match(original, cursor):
                if path := PATH.fullmatch(attribute.group(0)):
                    path_match = path
                cursor = attribute.end()
                while cursor < len(code) and code[cursor].isspace():
                    cursor += 1
            line = original.count("\n", 0, match.start()) + 1
            module = MODULE.match(code, cursor)
            if not module:
                included = INCLUDE.match(original, cursor)
                if included and CRATE.name == "edgion-gateway":
                    actual = (source.parent / included.group(1)).resolve()
                    support_root = (CRATE / "tests" / "support").resolve()
                    if not actual.is_relative_to(support_root) or not actual.is_file():
                        errors.append(f"{relative}:{line}: invalid test support include {actual}")
                    mounts.setdefault(actual, []).append(f"{relative}:{line}")
                    continue
                if USE.match(code, cursor):
                    # Imports and re-exports are declarations, not test bodies.
                    continue
                associated = ASSOCIATED_ITEM.match(code, cursor)
                if (
                    CRATE.name == "edgion-gateway"
                    and associated
                    and associated.group(1) in GATEWAY_ASSOCIATED_TEST_HELPERS.get(relative, ())
                ):
                    continue
                if CRATE.name == "edgion-gateway":
                    line_end = code.find("\n", cursor)
                    if line_end < 0:
                        line_end = len(code)
                    source_line = " ".join(code[cursor:line_end].split())
                    if source_line == "{":
                        following = next(
                            (" ".join(line.split()) for line in code[line_end + 1 :].splitlines() if line.strip()),
                            "",
                        )
                        source_line = "{ " + following
                    if source_line in GATEWAY_SOURCE_LOCAL.get(relative, ()):
                        continue
                exception = next(
                    (
                        key
                        for key in SCOPED_EXCEPTIONS
                        if key[0] == CRATE.name
                        and key[1] == relative
                        and key[2].match(code, cursor)
                    ),
                    None,
                )
                if exception:
                    continue
                line = original.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: test-only implementation remains in src")
                continue
            name, kind = module.groups()
            if kind != ";":
                errors.append(f"{relative}:{line}: inline test module {name}")
                continue
            if not path_match:
                errors.append(f"{relative}:{line}: missing #[path] for {name}")
                continue
            nested_annotations = (
                CRATE.name == "edgion-resources"
                and relative == Path("constants/annotations.rs")
                and name == "plugin_ref_list_tests"
            )
            # This declaration is inside annotations::edgion. Rust resolves its
            # path from the inline module's virtual directory.
            path_base = source.parent / "annotations" / "edgion" if nested_annotations else source.parent
            actual = (path_base / path_match.group(1)).resolve()
            base = CRATE / "tests" / "unit" / relative.parent / f"{relative.stem}_test"
            if name == "test_support" and CRATE.name == "edgion-common" and relative == Path("link_sys_core/outbound_tls.rs"):
                expected = support
            elif name == "test_support" and CRATE.name in ("edgion-controller", "edgion-waf"):
                if CRATE.name == "edgion-controller" and relative == Path("conf_mgr/conf_center/file_system/center.rs"):
                    expected = (CRATE / "tests" / "support" / "file_system_center.rs").resolve()
                else:
                    expected = (CRATE / "tests" / "support" / relative).resolve()
            elif nested_annotations:
                expected = (CRATE / "tests" / "unit" / "constants" / "annotations_test" / "edgion" / "plugin_ref_list_tests.rs").resolve()
            else:
                expected = (base.with_suffix(".rs") if name == "tests" else base / f"{name}.rs").resolve()
            if actual != expected:
                errors.append(f"{relative}:{line}: {name} mounts {actual}, expected {expected}")
            if not actual.is_file():
                errors.append(f"{relative}:{line}: missing {actual}")
            mounts.setdefault(actual, []).append(f"{relative}:{line}")
        for match in TEST.finditer(code):
            line = original.count("\n", 0, match.start()) + 1
            errors.append(f"{relative}:{line}: test attribute remains in src")
    for path, owners in mounts.items():
        if len(owners) != 1:
            errors.append(f"{path}: duplicate mounts: {', '.join(owners)}")
    if CRATE.name == "edgion-gateway":
        # Two pre-existing test directories contain child modules. Their
        # explicit paths keep Cargo's test identities after moving the parents.
        child = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]\s*mod\s+\w+\s*;')
        for parent in sorted((CRATE / "tests" / "unit").rglob("mod_test.rs")):
            for match in child.finditer(parent.read_text()):
                actual = (parent.parent / match.group(1)).resolve()
                if not actual.is_file():
                    errors.append(f"{parent}: missing child module {actual}")
                mounts.setdefault(actual, []).append(str(parent))
    for path in sorted((CRATE / "tests" / "unit").rglob("*.rs")):
        if path.resolve() not in mounts:
            errors.append(f"{path.relative_to(CRATE)}: orphan unit-test file")
    integration_support = set()
    support_root = (CRATE / "tests" / "support").resolve()
    integration_mount = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]\s*(?:pub\s+)?mod\s+\w+\s*;')
    for owner in sorted((CRATE / "tests").glob("*.rs")):
        original = owner.read_text()
        code = mask_non_code(original)
        for match in integration_mount.finditer(original):
            if code[match.start()] != "#":
                continue
            actual = (owner.parent / match.group(1)).resolve()
            # Cross-crate helpers are registered by their owning crate. Unit
            # files still require their production module's unique mount.
            if actual.is_relative_to(support_root):
                if actual.is_file():
                    integration_support.add(actual)
                else:
                    errors.append(f"{owner.relative_to(CRATE)}: missing test support {actual}")
    for path in sorted((CRATE / "tests" / "support").rglob("*.rs")):
        if path.resolve() not in mounts and path.resolve() not in integration_support:
            errors.append(f"{path.relative_to(CRATE)}: orphan support file")
    return errors


if __name__ == "__main__":
    failures = []
    mounted = 0
    for name in MIGRATED_CRATES:
        CRATE = ROOT / name
        failures.extend(check())
        mounted += len(list((CRATE / "tests" / "unit").rglob("*.rs")))
    if failures:
        print("\n".join(failures), file=sys.stderr)
        sys.exit(1)
    print(f"Unit-test layout passed: {mounted} mounted files across {len(MIGRATED_CRATES)} crates")
