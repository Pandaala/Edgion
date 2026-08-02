#!/usr/bin/env python3
"""Enforce the iron rule "Zero Tracing on the Data Plane".

The boundary is the per-request / per-connection HOT PATH, not the whole Gateway
binary. The Gateway also hosts control-plane-shaped code: receiving config pushed
down from the Controller and building runtime objects from it (routes_mgr /
l4_rebuild / conf_handler / *_store), plus background workers (discovery, health,
acme) and startup. That code legitimately uses `tracing`.

Crucially the boundary is FUNCTION-LEVEL, not file-level: a single plugin.rs holds
both `from_config` / `new` (control plane, tracing allowed) and the per-request
`run_request` (hot path, tracing forbidden). A file/regex gate cannot separate
them, so it would either miss the request methods (most plugins keep them in
plugin.rs) or false-alarm on constructors.

This scanner therefore works at the function level: within the canonical
"Data-plane entry list" files (see skills/03-coding/01-log-safety.md), it flags a
`tracing` use only when its ENCLOSING FUNCTION is NOT a constructor / config-build
/ once-init function. Constructors (new, from_config, create, build, load_*,
compile_*, init_*, parse_*, ...) are recognised by name and skipped; everything
else in these files is request/connection handling and must be tracing-free.

Rollout: ZERO-TOLERANCE. The covered hot path is already tracing-free, so any new
`tracing` in a request/connection function fails the gate. A genuine registered
exception (the exception table in 01-log-safety.md) carries an inline
`// tracing-allow: <reason>` marker on the offending line, which the gate skips.

Usage:
  check_gateway_no_tracing.py            CI gate (exit 1 on any violation)
  check_gateway_no_tracing.py --list     list current matches, exit 0

Policy source:
  skills/03-coding/01-log-safety.md  -> Iron rule 3 + "Data-plane entry list"
  skills/04-review/observability/observability-review-rule.md
"""

from __future__ import annotations

import glob
import os
import re
import sys

# Repo root = two levels up from cicd/checks/.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ROOT = "edgion-gateway/src"

# ---------------------------------------------------------------------------
# Knobs -- keep these explicit and at the top so changes are reviewable.
# ---------------------------------------------------------------------------

# Canonical data-plane entry list. Mirror of skills/03-coding/01-log-safety.md
# "Data-plane entry list". Paths are relative to ROOT.
#   DP_DIRS  -> scanned recursively (auto-covers new files, e.g. a new plugin)
#   DP_FILES -> individual files (their directory also holds config/build code)
#   DP_GLOBS -> shell globs
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
    # Request-path reachable via the OIDC lazy discovery / JWKS fetch. Its only
    # cold caller (the prewarm task) lives in plugin.rs, so nothing in this file
    # may trace.
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

# Never on the request path, even inside a DP dir: config-apply handlers, and
# wholly test-only helper files (declared `#[cfg(test)] mod ...` by their parent,
# so they carry no `mod tests` block of their own for TEST_MOD_RE to latch on).
NAME_SKIP = re.compile(r"conf_handler|test_helpers")

# A function whose name matches this is construction / config-build / once-init
# / trait-boilerplate -- tracing there is control-plane and allowed. Everything
# else in a data-plane entry file is request/connection handling. This is the
# reviewable knob: add a name here if a genuinely-cold function is flagged.
COLD_FN = re.compile(
    r"^(new|create|build|default|from_|with_|init|load|compile|parse|configure|"
    r"register|update|initialize|spawn|drop|fmt|clone|deserialize|serialize|"
    r"add_from_|remove_|begin_draining|prewarm)"
)

# Forbidden tracing usage. The `log` crate is intentionally not matched: the
# project ships its own access-log module literally named `log`
# (crate::plugins::runtime::log -> PluginLog), so any `log::` regex collides
# with the sanctioned path.
TRACING = re.compile(r"tracing::|use\s+tracing|#\[instrument|\b\w*_span!|\bevent!")

FN_RE = re.compile(r"\bfn\s+([a-z_][A-Za-z0-9_]*)")
# Trailing unit-test module, including the `<topic>_tests` naming several files use.
# Matching this (rather than any `#[cfg(test)]`) is what keeps the latch in
# collect_violations from firing on a test-only item mid-file.
TEST_MOD_RE = re.compile(r"\bmod\s+\w*tests?\b")
OPT_OUT = "tracing-allow:"


def iter_files():
    seen = set()
    for d in DP_DIRS:
        for f in glob.glob(os.path.join(ROOT, d, "**", "*.rs"), recursive=True):
            seen.add(f)
    for f in DP_FILES:
        p = os.path.join(ROOT, f)
        if os.path.isfile(p):
            seen.add(p)
    for g in DP_GLOBS:
        for f in glob.glob(os.path.join(ROOT, g)):
            seen.add(f)
    return sorted(seen)


def collect_violations():
    violations = []
    for path in iter_files():
        if NAME_SKIP.search(os.path.basename(path)):
            continue
        try:
            lines = open(path, encoding="utf-8").read().splitlines()
        except OSError:
            continue
        cur_fn = "?"
        in_test = False
        rel = os.path.relpath(path, ROOT)
        for i, line in enumerate(lines, 1):
            # `in_test` latches to EOF, so it must only trip on the trailing test
            # MODULE -- not on a bare `#[cfg(test)]` attribute over a single item.
            # A file with a test-only `use`/fn near the top (jwks_provider.rs) would
            # otherwise latch at that line and silently skip every later hot-path
            # tracing call, turning its gate coverage into decoration.
            if TEST_MOD_RE.search(line):
                in_test = True
            m = FN_RE.search(line)
            if m:
                cur_fn = m.group(1)
            if not TRACING.search(line):
                continue
            if OPT_OUT in line:
                continue
            if in_test:
                continue
            if COLD_FN.match(cur_fn):
                continue
            violations.append((rel, i, cur_fn, line.strip()))
    return violations


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

    violations = collect_violations()

    if mode == "--list":
        if not violations:
            print("No tracing in data-plane request/connection functions.")
            return 0
        for rel, ln, fn, code in violations:
            print(f"{rel}:{ln}: [fn {fn}] {code}")
        files = len({v[0] for v in violations})
        print(f"---- {len(violations)} line(s) across {files} file(s).")
        return 0

    if violations:
        sys.stderr.write(
            "ERROR: tracing found in data-plane request/connection functions "
            "(must be 0):\n"
        )
        for rel, ln, fn, code in violations:
            sys.stderr.write(f"  {rel}:{ln}: [fn {fn}] {code}\n")
        sys.stderr.write(
            "\n"
            "  Replace with PluginLog -> access log, ctx.add_error()/ctx.err_log, the\n"
            "  typed async loggers (ssl_log/tls_log/tcp_log/udp_log), or global_metrics().\n"
            "  If the enclosing function is genuinely construction/config-time, either\n"
            "  rename it to a cold prefix or add its name to COLD_FN in this script.\n"
            "  For a registered exception, add an inline '// tracing-allow: <reason>' on\n"
            "  the line and record it in the exception table of 01-log-safety.md.\n"
        )
        return 1

    print("OK: gateway data-plane request/connection functions are tracing-free.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
