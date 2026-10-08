#!/usr/bin/env python3
"""Enforce the iron rule "Zero Tracing on the Data Plane".

The boundary is the per-request / per-connection hot path, not the whole
Gateway binary. Files in the canonical data-plane candidate set may contain
both runtime callbacks and configuration-time construction. Inside that set,
every tracing use is treated as hot by default.

Cold functions are exceptions to that default and must be registered by exact
``(relative path, module-qualified impl owner, function name)`` with a reviewable reason. Function-name
prefixes are deliberately not trusted: request-time helpers can legitimately
be named ``build_headers`` or ``parse_authorization_value``. Registry entries
are checked in both directions, so a renamed function or a function that no
longer traces makes the guard fail until the stale exception is removed.

Genuine hot-path exceptions use an inline ``// tracing-allow: <reason>`` on the
tracing line and must also be recorded in the exception table in
``skills/03-coding/01-log-safety.md``.

Usage:
  check_gateway_no_tracing.py            CI gate (exit 1 on any finding)
  check_gateway_no_tracing.py --list     list current findings, exit 0

Policy source:
  skills/03-coding/01-log-safety.md  -> Iron rule 3 + Data-plane entry list
  skills/04-review/observability/mandatory-design/observability-review-rule.md
"""

from __future__ import annotations

import glob
import os
import re
import sys
from collections import Counter
from dataclasses import dataclass


REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ROOT = "edgion-gateway/src"
SOURCE_ROOT = os.path.join(REPO_ROOT, ROOT)

# Canonical data-plane candidate set. Keep synchronized with
# skills/03-coding/01-log-safety.md.
DP_DIRS = [
    "routes/http/proxy_http",
    "routes/http/match_engine",
    "plugins/runtime/gateway_api_filters",
    "plugins/runtime/traits",
    "plugins/stream/tls_route",
    "plugins/stream/ip_restriction",
    "plugins/stream/global_connection_ip_restriction",
    "lb/selection",
    "lb/runtime_state",
    "backends/policy",
    "cache",
    "plugins/http/common/outbound",
]
DP_FILES = [
    "routes/http/match_unit.rs",
    "routes/http/session_persistence.rs",
    "routes/http/redirect_http.rs",
    "routes/grpc/match_engine.rs",
    "routes/grpc/match_unit.rs",
    "routes/grpc/integration.rs",
    "routes/tcp/edgion_tcp.rs",
    "routes/tls/passthrough_proxy.rs",
    "routes/tls/proxy.rs",
    "routes/udp/edgion_udp.rs",
    "tls/runtime/gateway/tls_pingora.rs",
    "tls/runtime/backend/backend_api.rs",
    "tls/runtime/backend/cert_extractor.rs",
    "tls/boringssl/mtls_verify_callback.rs",
    "tls/store/cert_matcher.rs",
    "plugins/http/openid_connect/jwks_provider.rs",
    "plugins/runtime/plugin_runtime.rs",
    "plugins/runtime/conditional_filter.rs",
    "plugins/runtime/reject_filter.rs",
    "plugins/runtime/session_adapter.rs",
    "plugins/stream/stream_plugin_runtime.rs",
    "plugins/stream/connection_filter_bridge.rs",
    "backends/mod.rs",
    "backends/validation.rs",
    "backends/circuit_breaker/store.rs",
    "runtime/server/error_response.rs",
]
DP_GLOBS = [
    "plugins/http/*/plugin.rs",
    "plugins/http/*/request_filter_impl.rs",
    "plugins/http/*/request_stage.rs",
    "plugins/stream/*/plugin.rs",
]

# Exact cold-path registrations. These are configuration construction,
# configuration apply, one-time initialization, or background-task setup.
# Never register a request/connection callback here.
COLD_FUNCTION_REGISTRATIONS = [
    (
        "backends/policy/backend_tls/conf_handler_impl.rs",
        "ConfHandler<BackendTLSPolicy> for BackendTLSPolicyStore",
        "full_set",
        "configuration snapshot apply",
    ),
    (
        "backends/policy/backend_tls/conf_handler_impl.rs",
        "ConfHandler<BackendTLSPolicy> for BackendTLSPolicyStore",
        "partial_update",
        "configuration snapshot apply",
    ),
    (
        "backends/policy/backend_traffic/conf_handler_impl.rs",
        "ConfHandler<EdgionBackendTrafficPolicy> for BackendTrafficPolicyStore",
        "full_set",
        "configuration snapshot apply",
    ),
    (
        "backends/policy/backend_traffic/conf_handler_impl.rs",
        "ConfHandler<EdgionBackendTrafficPolicy> for BackendTrafficPolicyStore",
        "partial_update",
        "configuration snapshot apply",
    ),
    ("lb/runtime_state/mod.rs", "<free>", "begin_draining", "backend lifecycle transition"),
    ("lb/runtime_state/mod.rs", "<free>", "remove_service", "backend lifecycle transition"),
    ("lb/selection/round_robin.rs", "RoundRobinSelector", "build", "load-balancer construction"),
    (
        "plugins/http/all_endpoint_status/plugin.rs",
        "<free>",
        "init_all_endpoint_status_global_config",
        "plugin configuration initialization",
    ),
    ("plugins/http/basic_auth/plugin.rs", "BasicAuth", "load_users", "plugin construction"),
    ("plugins/http/common/outbound/egress.rs", "<free>", "configure_egress_defaults", "configuration initialization"),
    ("plugins/http/cors/plugin.rs", "Cors", "new", "plugin construction"),
    ("plugins/http/dsl/plugin.rs", "DslPlugin", "from_config", "plugin construction"),
    ("plugins/http/dsl/plugin.rs", "DslPlugin", "from_config_response", "plugin construction"),
    ("plugins/http/ext_proc/plugin.rs", "ExtProc", "build_with_identity", "plugin construction"),
    ("plugins/http/geoip_location/plugin.rs", "GeoIpLocation", "create", "plugin construction"),
    ("plugins/http/header_cert_auth/plugin.rs", "HeaderCertAuth", "load_ca_certs", "plugin construction"),
    ("plugins/http/ip_restriction/plugin.rs", "IpRestriction", "create_bound", "plugin construction"),
    ("plugins/http/key_auth/plugin.rs", "KeyAuth", "create", "plugin construction"),
    ("plugins/http/ldap_auth/plugin.rs", "LdapAuth", "create", "plugin construction"),
    ("plugins/http/ldap_auth/plugin.rs", "LdapAuth", "new", "plugin construction"),
    ("plugins/http/openid_connect/plugin.rs", "OpenidConnect", "try_new", "fallible plugin construction"),
    ("plugins/http/openid_connect/plugin.rs", "<free>", "spawn_discovery_prewarm", "bounded background prewarm setup"),
    ("plugins/http/real_ip/plugin.rs", "RealIp", "create", "plugin construction"),
    ("plugins/http/request_access_policy/plugin.rs", "RequestAccessPolicy", "build", "plugin construction"),
    ("plugins/http/request_mirror/plugin.rs", "RequestMirrorPlugin", "new", "plugin construction"),
    ("plugins/http/wasm/plugin.rs", "WasmPlugin", "build", "plugin construction"),
    ("plugins/runtime/plugin_runtime.rs", "PluginRuntime", "add_request_entries", "configuration publication assembles cached plugin entries"),
    (
        "plugins/stream/ip_restriction/stream_ip_restriction.rs",
        "<free>",
        "init_ip_restriction",
        "plugin configuration initialization",
    ),
    (
        "plugins/stream/connection_rate_limit/plugin.rs",
        "ConnectionRateLimitPlugin",
        "new",
        "plugin construction",
    ),
    (
        "plugins/stream/stream_plugin_runtime.rs",
        "StreamPluginRuntime",
        "from_owner_cached",
        "configuration publication assembles cached stream entries",
    ),
    ("plugins/stream/tls_route/ip_restriction.rs", "TlsRouteIpRestriction", "from_bound", "plugin construction"),
    ("routes/grpc/match_unit.rs", "GrpcMatchInfo", "new", "route matcher construction"),
    ("routes/http/match_engine/radix_path.rs", "RadixPath", "new", "route matcher construction"),
    (
        "routes/http/match_engine/radix_route_match.rs",
        "RadixRouteMatchEngine",
        "initialize_internal",
        "route matcher construction",
    ),
    ("routes/http/match_engine/regex_routes_engine.rs", "RegexRoutesEngine", "build", "route matcher construction"),
    ("routes/http/match_unit.rs", "HttpRouteRuleUnit", "compile_match_regexes", "route matcher construction"),
    (
        "routes/http/proxy_http/parse_timeout.rs",
        "ParsedClientTimeout",
        "from_config",
        "listener configuration parsing",
    ),
    (
        "routes/http/proxy_http/parse_timeout.rs",
        "ParsedBackendTimeout",
        "from_config",
        "listener configuration parsing",
    ),
    ("tls/boringssl/mtls_verify_callback.rs", "<free>", "init_ex_data_idx", "process-wide callback initialization"),
    (
        "tls/runtime/gateway/tls_pingora.rs",
        "ssl_ctx_ex_data::<free>",
        "init_ssl_ctx_ex_data_idx",
        "process-wide callback initialization",
    ),
]

KNOWN_HOT_FUNCTIONS = {
    "apply", "certificate_callback", "connected_to_upstream", "early_request_filter",
    "error_while_proxy", "fail_to_connect", "fail_to_proxy", "filter",
    "handle_client_packet", "handle_upstream_packets", "handshake_complete_callback",
    "logging", "on_request", "process_new", "request_body_filter", "request_filter",
    "response_filter", "serve", "upstream_peer", "upstream_response_body_filter",
    "upstream_response_filter", "verify_callback", "verify_callback_impl",
}

TRACING = re.compile(r"tracing::|use\s+tracing|#\s*\[\s*instrument\b|\b\w*_span!|\bevent!")
FN_RE = re.compile(r"\bfn\s+([a-z_][A-Za-z0-9_]*)\b")
IMPL_RE = re.compile(r"(?:^\s*|(?<=[{};])\s*)impl\b", re.MULTILINE)
MODULE_RE = re.compile(
    r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{",
    re.MULTILINE,
)
TEST_MOD_RE = re.compile(r"\bmod\s+\w*tests?\b")
ATTRIBUTE_BLOCK_RE = re.compile(r"(?:#\s*\[[^\]]*\]\s*)+$", re.DOTALL)
OPT_OUT = "tracing-allow:"


@dataclass(frozen=True)
class FunctionSpan:
    owner: str
    name: str
    start: int
    end: int
    test_only: bool


@dataclass
class SourceScan:
    violations: list[tuple[str, int, str, str]]
    functions: list[tuple[str, str]]
    tracing_functions: set[tuple[str, str]]


@dataclass(frozen=True)
class ImplSpan:
    owner: str
    start: int
    end: int


@dataclass(frozen=True)
class ModuleSpan:
    name: str
    start: int
    end: int


@dataclass(frozen=True)
class LexedRust:
    clean: str
    line_comments: tuple[tuple[int, int], ...]


def iter_files():
    seen = set()
    for directory in DP_DIRS:
        seen.update(glob.glob(os.path.join(SOURCE_ROOT, directory, "**", "*.rs"), recursive=True))
    for filename in DP_FILES:
        path = os.path.join(SOURCE_ROOT, filename)
        if os.path.isfile(path):
            seen.add(path)
    for pattern in DP_GLOBS:
        seen.update(glob.glob(os.path.join(SOURCE_ROOT, pattern)))
    return sorted(seen)


def _blank(chars: list[str], start: int, end: int) -> None:
    for index in range(start, end):
        if chars[index] != "\n":
            chars[index] = " "


def lex_rust(source: str) -> LexedRust:
    """Blank comments and literals while preserving byte offsets/newlines."""
    chars = list(source)
    line_comments = []
    length = len(source)
    index = 0
    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = length if end < 0 else end
            line_comments.append((index, end))
            _blank(chars, index, end)
            index = end
            continue
        if source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < length and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            _blank(chars, index, end)
            index = end
            continue

        raw_prefix = None
        for prefix in ("br", "cr", "r"):
            if source.startswith(prefix, index):
                cursor = index + len(prefix)
                while cursor < length and source[cursor] == "#":
                    cursor += 1
                if cursor < length and source[cursor] == '"':
                    raw_prefix = (cursor - index - len(prefix), cursor + 1)
                    break
        if raw_prefix is not None:
            hash_count, body_start = raw_prefix
            terminator = '"' + ("#" * hash_count)
            found = source.find(terminator, body_start)
            end = length if found < 0 else found + len(terminator)
            _blank(chars, index, end)
            index = end
            continue

        quote_start = index + 1 if source.startswith(('b"', 'c"'), index) else index
        if source[quote_start:quote_start + 1] == '"':
            end = quote_start + 1
            while end < length:
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            _blank(chars, index, min(end, length))
            index = min(end, length)
            continue

        char_start = index + 1 if source.startswith("b'", index) else index
        if source[char_start:char_start + 1] == "'":
            end = char_start + 1
            end += 2 if end < length and source[end] == "\\" else 1
            if end < length and source[end] == "'":
                end += 1
                _blank(chars, index, end)
                index = end
                continue
        index += 1
    return LexedRust("".join(chars), tuple(line_comments))


def sanitize_rust(source: str) -> str:
    return lex_rust(source).clean


def _matching_brace(source: str, opening: int) -> int | None:
    depth = 0
    for index in range(opening, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    return None


def _body_span(source: str, start: int) -> tuple[int, int] | None:
    opening = source.find("{", start)
    semicolon = source.find(";", start)
    if opening < 0 or (semicolon >= 0 and semicolon < opening):
        return None
    end = _matching_brace(source, opening)
    return None if end is None else (opening, end)


def _test_module_spans(source: str) -> list[tuple[int, int]]:
    spans = []
    for match in TEST_MOD_RE.finditer(source):
        if not _has_cfg_test_attribute(source, match.start()):
            continue
        body = _body_span(source, match.end())
        if body is not None:
            spans.append((match.start(), body[1]))
    return spans


def _inside(offset: int, spans: list[tuple[int, int]]) -> bool:
    return any(start <= offset < end for start, end in spans)


def _has_cfg_test_attribute(source: str, item_start: int) -> bool:
    prefix = re.sub(r"pub(?:\s*\([^)]*\))?\s*$", "", source[:item_start])
    attributes = ATTRIBUTE_BLOCK_RE.search(prefix)
    if attributes is None:
        return False
    block = attributes.group(0)
    for cfg in re.finditer(r"#\s*\[\s*cfg\s*\(", block):
        opening = block.find("(", cfg.start())
        closing = _matching_parenthesis(block, opening)
        if closing is not None and _cfg_requires_test(block[opening + 1:closing - 1]):
            return True
    return False


def _matching_parenthesis(source: str, opening: int) -> int | None:
    depth = 0
    for index in range(opening, len(source)):
        if source[index] == "(":
            depth += 1
        elif source[index] == ")":
            depth -= 1
            if depth == 0:
                return index + 1
    return None


def _split_cfg_arguments(source: str) -> list[str]:
    arguments = []
    depth = 0
    start = 0
    for index, char in enumerate(source):
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        elif char == "," and depth == 0:
            arguments.append(source[start:index].strip())
            start = index + 1
    arguments.append(source[start:].strip())
    return [argument for argument in arguments if argument]


def _cfg_possibilities(expression: str) -> tuple[bool, bool]:
    """Return whether a cfg expression can be false/true when test=false."""
    expression = expression.strip()
    if expression == "test":
        return True, False
    call = re.fullmatch(r"(all|any|not)\s*\((.*)\)", expression, re.DOTALL)
    if call is None:
        return True, True
    operator, body = call.groups()
    children = [_cfg_possibilities(argument) for argument in _split_cfg_arguments(body)]
    if operator == "not" and len(children) == 1:
        can_false, can_true = children[0]
        return can_true, can_false
    if operator == "all":
        return any(can_false for can_false, _ in children), all(can_true for _, can_true in children)
    if operator == "any":
        return all(can_false for can_false, _ in children), any(can_true for _, can_true in children)
    return True, True


def _cfg_requires_test(expression: str) -> bool:
    _can_false, can_true = _cfg_possibilities(expression)
    return not can_true


def _impl_spans(source: str) -> list[ImplSpan]:
    spans = []
    for match in IMPL_RE.finditer(source):
        body = _body_span(source, match.end())
        if body is None:
            continue
        header = " ".join(source[match.end():body[0]].split())
        spans.append(ImplSpan(header, match.start(), body[1]))
    return spans


def _enclosing_impl(offset: int, impls: list[ImplSpan]) -> ImplSpan | None:
    candidates = [impl for impl in impls if impl.start <= offset < impl.end]
    return min(candidates, key=lambda impl: impl.end - impl.start) if candidates else None


def _module_spans(source: str) -> list[ModuleSpan]:
    spans = []
    for match in MODULE_RE.finditer(source):
        opening = source.find("{", match.start(), match.end())
        end = _matching_brace(source, opening)
        if end is not None:
            spans.append(ModuleSpan(match.group(1), match.start(), end))
    return spans


def _module_path(offset: int, modules: list[ModuleSpan]) -> str:
    enclosing = [module for module in modules if module.start <= offset < module.end]
    enclosing.sort(key=lambda module: module.end - module.start, reverse=True)
    return "::".join(module.name for module in enclosing)


def function_spans(source: str) -> list[FunctionSpan]:
    test_modules = _test_module_spans(source)
    impls = _impl_spans(source)
    modules = _module_spans(source)
    spans = []
    for match in FN_RE.finditer(source):
        body = _body_span(source, match.end())
        if body is None:
            continue
        test_only = _inside(match.start(), test_modules) or _has_cfg_test_attribute(source, match.start())
        impl = _enclosing_impl(match.start(), impls)
        owner = impl.owner if impl is not None else "<free>"
        module_path = _module_path(match.start(), modules)
        if module_path:
            owner = f"{module_path}::{owner}"
        spans.append(FunctionSpan(owner, match.group(1), match.start(), body[1], test_only))
    return spans


def _enclosing_function(offset: int, functions: list[FunctionSpan]) -> FunctionSpan | None:
    candidates = [function for function in functions if function.start <= offset < function.end]
    return min(candidates, key=lambda function: function.end - function.start) if candidates else None


def _line_at(source: str, offset: int) -> tuple[int, str]:
    line_number = source.count("\n", 0, offset) + 1
    line_start = source.rfind("\n", 0, offset) + 1
    line_end = source.find("\n", offset)
    if line_end < 0:
        line_end = len(source)
    return line_number, source[line_start:line_end].strip()


def _has_inline_opt_out(source: str, match_offset: int, comments: tuple[tuple[int, int], ...]) -> bool:
    line_end = source.find("\n", match_offset)
    if line_end < 0:
        line_end = len(source)
    for start, end in comments:
        if match_offset < start < line_end:
            comment = source[start + 2:end]
            marker = comment.find(OPT_OUT)
            return marker >= 0 and bool(comment[marker + len(OPT_OUT):].strip())
    return False


def scan_source(relative_path: str, source: str, cold_keys: set[tuple[str, str, str]]) -> SourceScan:
    lexed = lex_rust(source)
    clean = lexed.clean
    functions = function_spans(clean)
    violations = []
    tracing_functions = set()
    for match in TRACING.finditer(clean):
        function = _enclosing_function(match.start(), functions)
        if function is not None and function.test_only:
            continue
        line_number, line = _line_at(source, match.start())
        if _has_inline_opt_out(source, match.start(), lexed.line_comments):
            continue
        function_name = function.name if function is not None else "?"
        owner = function.owner if function is not None else "<module>"
        identity = (owner, function_name)
        tracing_functions.add(identity)
        if (relative_path, owner, function_name) in cold_keys:
            continue
        display_name = f"{owner}::{function_name}"
        violations.append((relative_path, line_number, display_name, line))
    return SourceScan(
        violations,
        [(function.owner, function.name) for function in functions],
        tracing_functions,
    )


def _registration_state(registrations=COLD_FUNCTION_REGISTRATIONS):
    keys = [(path, owner, function) for path, owner, function, _reason in registrations]
    duplicate_keys = sorted({key for key in keys if keys.count(key) > 1})
    return set(keys), duplicate_keys


def validate_registrations(
    registrations,
    scanned_paths: set[str],
    functions_by_path: dict[str, list[tuple[str, str]]],
    tracing_by_path: dict[str, set[tuple[str, str]]],
) -> list[str]:
    _cold_keys, duplicate_keys = _registration_state(registrations)
    errors = [
        f"duplicate cold-function registration: {path}::{owner}::{function}"
        for path, owner, function in duplicate_keys
    ]
    for relative_path, owner, function, reason in registrations:
        identity = (owner, function)
        display_name = f"{relative_path}::{owner}::{function}"
        if not reason.strip():
            errors.append(f"cold-function registration has no reason: {display_name}")
        if function in KNOWN_HOT_FUNCTIONS or function.startswith("run_"):
            errors.append(f"known hot function cannot be registered cold: {display_name}")
        if relative_path not in scanned_paths:
            errors.append(f"cold-function file is outside the scan set or missing: {relative_path}")
            continue
        function_count = Counter(functions_by_path.get(relative_path, [])).get(identity, 0)
        if function_count == 0:
            errors.append(f"stale cold-function registration (function missing): {display_name}")
        elif function_count > 1:
            errors.append(
                "ambiguous cold-function registration "
                f"({function_count} matching items): {display_name}"
            )
        elif identity not in tracing_by_path.get(relative_path, set()):
            errors.append(f"stale cold-function registration (no tracing remains): {display_name}")
    return errors


def scan_repository():
    cold_keys, _duplicate_keys = _registration_state()
    violations = []
    registry_errors = []
    functions_by_path = {}
    tracing_by_path = {}
    scanned_paths = set()

    for path in iter_files():
        relative_path = os.path.relpath(path, SOURCE_ROOT)
        scanned_paths.add(relative_path)
        try:
            source = open(path, encoding="utf-8").read()
        except OSError as error:
            registry_errors.append(f"cannot read scanned file {relative_path}: {error}")
            continue
        result = scan_source(relative_path, source, cold_keys)
        violations.extend(result.violations)
        functions_by_path[relative_path] = result.functions
        tracing_by_path[relative_path] = result.tracing_functions

    registry_errors.extend(
        validate_registrations(
            COLD_FUNCTION_REGISTRATIONS,
            scanned_paths,
            functions_by_path,
            tracing_by_path,
        )
    )

    return violations, registry_errors


def collect_violations():
    """Compatibility helper for callers interested only in hot-path matches."""
    return scan_repository()[0]


def main(argv):
    os.chdir(REPO_ROOT)
    mode = argv[1] if len(argv) > 1 else "--check"
    if mode in ("-h", "--help"):
        print(__doc__)
        return 0
    if mode not in ("--check", "--list", ""):
        sys.stderr.write(f"Unknown argument: {mode}\n")
        sys.stderr.write("Usage: check_gateway_no_tracing.py [--check | --list]\n")
        return 2

    violations, registry_errors = scan_repository()
    if mode == "--list":
        for relative_path, line, function, code in violations:
            print(f"{relative_path}:{line}: [fn {function}] {code}")
        for error in registry_errors:
            print(f"REGISTRY: {error}")
        print(f"---- {len(violations)} tracing violation(s), {len(registry_errors)} registry error(s).")
        return 0

    if violations or registry_errors:
        if violations:
            sys.stderr.write("ERROR: tracing found in data-plane request/connection functions:\n")
            for relative_path, line, function, code in violations:
                sys.stderr.write(f"  {relative_path}:{line}: [fn {function}] {code}\n")
        if registry_errors:
            sys.stderr.write("ERROR: invalid or stale cold-function registrations:\n")
            for error in registry_errors:
                sys.stderr.write(f"  {error}\n")
        sys.stderr.write(
            "\nUse PluginLog/access log/context diagnostics/typed async loggers/metrics on the hot path.\n"
            "Register a function as cold only after proving its call lifecycle, with an exact path,\n"
            "exact module-qualified impl owner, exact function name, and reason.\n"
            "Reserve tracing-allow for documented hot exceptions.\n"
        )
        return 1

    print("OK: gateway data-plane tracing boundary and cold-function registry are clean.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
