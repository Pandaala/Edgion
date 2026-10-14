#!/usr/bin/env python3
"""Guard: the metrics inventory doc must match the metric names emitted in code.

Bidirectional: a metric in code but not the inventory (silent-drift bug), or an
inventory row naming no emitted metric (stale doc), both fail. See
docs/en/ops-guide/observability/metrics-inventory.md and the design spec
docs/superpowers/specs/2026-07-20-metrics-observability-hardening-design.md.
"""
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
INVENTORY = REPO / "docs/en/ops-guide/observability/metrics-inventory.md"

SCANNED = [
    "edgion-gateway/src/observe/metrics/registry.rs",
    "edgion-gateway/src/observe/metrics/route_handles.rs",
    "edgion-common/src/observe/fed_metrics.rs",
    "edgion-common/src/observe/system_metrics.rs",
    "edgion-common/src/observe/cardinality_guard.rs",
    "edgion-common/src/conf_sync/cert_expiry_watch.rs",
    "edgion-controller/src/observe/status_metrics.rs",
    "edgion-controller/src/observe/unknown_fields_registry.rs",
    "edgion-controller/src/conf_mgr/sync_runtime/metrics.rs",
]

CONST_RE = re.compile(r"const\s+\w+\s*:\s*&\s*(?:'[a-z_]+\s+)?str\s*=\s*\"(edgion_[a-z0-9_]+)\"")
MACRO_LITERAL_RE = re.compile(
    r'(?:metrics::|edgion_common::observe::metrics_facade::)?(?:(?:fixed|dynamic)_)?(?:counter|gauge|histogram)!\(\s*"(edgion_[a-z0-9_]+)"'
)

COMMENT_RE = re.compile(r'//[^\n]*|/\*.*?\*/', re.DOTALL)

DANGEROUS_LABELS = {"path", "ip", "user_id", "trace_id", "host", "url", "uri", "query"}
# metric_name::label pairs that are knowingly acceptable, each with a reason.
LABEL_ALLOWLIST = {
    # e.g. "edgion_example_total::host": "bounded to N configured upstreams",
}

# Matches the macro invocation head only, up to and including the open paren;
# the argument list itself is captured separately via `extract_balanced_args`
# below because the first argument is usually a `names::CONST` (or bare
# identifier), not a string literal, and later arguments may contain nested
# calls with their own parens (e.g. `"user" => normalize(p)`), which a
# non-greedy `(.+?)\)` would stop at prematurely.
MACRO_CALL_RE = re.compile(r'(?:metrics::)?(?:(?:fixed|dynamic)_)?(?:counter|gauge|histogram)!\(')
LABEL_KEY_RE = re.compile(r'"([a-z_]+)"\s*=>')


def extract_balanced_args(text, start):
    """Return the substring of `text` from `start` (the index just after an
    opening '(') up to its matching closing ')', tracking paren depth and
    ignoring parens that appear inside double-quoted string literals (so a
    label value like `"a)b"` doesn't desync the depth count). Returns None if
    the text ends before the matching paren is found (malformed/truncated
    input).
    """
    depth = 1
    in_string = False
    i = start
    n = len(text)
    while i < n:
        c = text[i]
        if in_string:
            if c == '\\':
                i += 1  # skip the escaped character
            elif c == '"':
                in_string = False
        elif c == '"':
            in_string = True
        elif c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return text[start:i]
        i += 1
    return None


def strip_comments(text):
    """Remove Rust line comments (// to end of line) and block comments
    (/* ... */, non-greedy, across lines). A single left-to-right alternation
    is used (rather than stripping block comments first, then line comments)
    so a `//` line comment that happens to contain a stray `/*` is matched as
    a line comment before the block-comment branch can misinterpret it as a
    real opener and swallow real code up to the next `*/`. Regex-based, not
    a full Rust lexer: a string literal that itself contains `//` or `/*`
    inside a comment is not specially handled, and that's acceptable here.
    """
    return COMMENT_RE.sub("", text)


# Only these four implementation files may own raw exporter handles/registration.
RAW_BACKEND_FILES = {
    "edgion-common/src/observe/cardinality_guard.rs",
    "edgion-common/src/observe/dynamic_metrics.rs",
    "edgion-common/src/observe/metrics_api.rs",
    "edgion-common/src/observe/metrics_facade.rs",
}
RAW_SYMBOLS = {"counter", "gauge", "histogram", "Counter", "Gauge", "Histogram",
               "CounterFn", "GaugeFn", "HistogramFn", "Recorder"}
EXPORTER_SYMBOLS = {"PrometheusBuilder", "PrometheusRecorder", "PrometheusHandle"}
# Mask strings as well as comments, preserving newlines. Raw API words inside a
# diagnostic/help literal are not Rust uses. This is a bounded lexical guard,
# not a general Rust parser; imports and ordinary aliases are handled below.
RUST_NON_CODE = re.compile(r'r(?P<hashes>\#{0,16})".*?"(?P=hashes)|"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/', re.DOTALL)


def rust_code(text):
    return RUST_NON_CODE.sub(lambda match: "".join("\n" if char == "\n" else " " for char in match.group()), text)


def raw_interface_bypass(text):
    code = rust_code(text)
    roots = {"metrics", "metrics_exporter_prometheus"}
    imports = re.findall(r"\buse\s+([^;]+);", code)
    for _ in range(len(imports)+1):
        before=set(roots)
        for tree in imports:
            for root in tuple(roots):
                roots.update(re.findall(r"\b" + re.escape(root) + r"\s+as\s+(\w+)", tree))
                for body in re.findall(r"\b" + re.escape(root) + r"\s*::\s*\{([^;]+)\}", tree):
                    roots.update(re.findall(r"\bself\s+as\s+(\w+)", body))
        if roots==before:
            break
    roots.update(re.findall(r"\bextern\s+crate\s+metrics\s+as\s+(\w+)", code))
    symbols = "|".join(sorted(RAW_SYMBOLS | EXPORTER_SYMBOLS))
    for root in roots:
        prefix = r"\b" + re.escape(root)
        if re.search(prefix + r"\s*::\s*(?:" + symbols + r")\b", code):
            return True
        for tree in imports:
            for body in re.findall(prefix + r"\s*::\s*\{([^;]+)\}", tree):
                if "*" in body or re.search(r"\b(?:" + symbols + r")\b", body):
                    return True
            if re.search(prefix + r"\s*::\s*\*", tree):
                return True
    if re.search(r"\bregister_(?:counter|gauge|histogram)\s*\(", code):
        return True
    if re.search(r"#\s*\[\s*macro_use\s*\]\s*extern\s+crate\s+metrics\b", code):
        return True
    return False


def production_sources(root):
    # Every workspace product/library src tree, including CLI and leaf crates;
    # edgion-tests is the dedicated integration harness, not production.
    root=Path(root)
    workspace=re.search(r"(?ms)^\[workspace\]\s*(.*?)(?=^\[|\Z)",(root/"Cargo.toml").read_text()).group(1)
    member_array=re.search(r"(?ms)^\s*members\s*=\s*\[(.*?)\]",workspace).group(1)
    members=re.findall(r'"([^"\n]+)"',member_array)
    return sorted(source for member in members for crate in root.glob(member)
                  if crate.name != "edgion-tests" and (crate / "src").is_dir()
                  for source in (crate / "src").rglob("*.rs"))


def guard_self_test():
    rejected = [
        'metrics::counter!("event");',
        'use metrics::counter; counter!("event");',
        'use metrics::{counter as events}; events!("event");',
        'use metrics::Counter as Cached; struct Cache { handle: Cached }',
        'use metrics as raw; raw::Gauge::noop();',
        'use second::Counter; use raw as second; use metrics as raw;',
        'use metrics::{self as raw, Key}; use raw::{Histogram as Cached};',
        'use metrics::*;',
        'use metrics::{Key, gauge as state};',
        'extern crate metrics as raw; raw::Counter::noop();',
        '#[macro_use] extern crate metrics; counter!("event");',
        'recorder.register_counter(&key, &metadata);',
        'metrics::Recorder::register_histogram(&recorder, &key, &metadata);',
        'use metrics_exporter_prometheus::PrometheusBuilder;',
    ]
    accepted = [
        'use metrics::{Key, Label};',
        'use edgion_common::observe::metrics_facade::{dynamic_counter as counter, DynamicCounter as Counter}; counter!("event");',
        'use edgion_common::observe::metrics_facade::FixedCounter; fixed_counter!("event");',
        'use metrics_exporter_prometheus::Matcher;',
        '// metrics::Counter\nlet help = "metrics::counter!";',
        'let help = r#"use metrics::Counter;"#;',
    ]
    for fixture in rejected:
        assert raw_interface_bypass(fixture), "raw interface escaped guard: " + fixture
    for fixture in accepted:
        assert not raw_interface_bypass(fixture), "canonical interface rejected: " + fixture
    assert "edgion-common/src/observe/system_metrics.rs" not in RAW_BACKEND_FILES
    for source in production_sources(REPO):
        assert source.parts[-1].endswith(".rs")
    print("OK: raw-interface imported/aliased/type/wildcard fixtures passed.")


def extract_metric_names_from_files(paths):
    names = set()
    for p in paths:
        p = Path(p)
        if not p.exists():
            continue
        text = strip_comments(p.read_text())
        names.update(CONST_RE.findall(text))
        names.update(MACRO_LITERAL_RE.findall(text))
    return names


def scan_dangerous_labels(paths):
    hits = []
    for p in paths:
        p = Path(p)
        if not p.exists():
            continue
        text = strip_comments(p.read_text())
        for m in MACRO_CALL_RE.finditer(text):
            args = extract_balanced_args(text, m.end())
            if args is None:
                continue
            # The first arg is the metric identifier: either a `names::CONST`
            # / bare identifier, or an `"edgion_..."` string literal. We don't
            # need to resolve it to a name here — reporting whatever token was
            # captured (const path or literal) is enough to locate the site.
            first_arg = args.split(",", 1)[0].strip()
            if first_arg.startswith('"') and first_arg.endswith('"'):
                first_arg = first_arg[1:-1]  # unquote a string-literal metric name
            for key in LABEL_KEY_RE.findall(args):
                if key in DANGEROUS_LABELS and (first_arg + "::" + key) not in LABEL_ALLOWLIST:
                    hits.append((first_arg, key))
    return hits


def extract_metric_names(root):
    return extract_metric_names_from_files([Path(root) / s for s in SCANNED])


def parse_inventory(md):
    names = set()
    md = Path(md)
    if not md.exists():
        return names
    for line in md.read_text().splitlines():
        m = re.match(r'\|\s*(edgion_[a-z0-9_]+)\s*\|', line)
        if m:
            names.add(m.group(1))
    return names


def diff(code, inv):
    missing = sorted(code - inv)  # in code, not documented
    stale = sorted(inv - code)  # in doc, not emitted
    return missing, stale


def main():
    code = extract_metric_names(REPO)
    inv = parse_inventory(INVENTORY)
    missing, stale = diff(code, inv)
    if missing:
        print("METRICS MISSING FROM INVENTORY (add rows for these):")
        for n in missing:
            print("  + " + n)
    if stale:
        print("STALE INVENTORY ROWS (no such metric emitted):")
        for n in stale:
            print("  - " + n)
    if missing or stale:
        return 1
    catalog = (REPO / "edgion-common/src/observe/metrics_catalog.rs").read_text()
    catalog_names = re.findall(r'Definition\s*\{\s*name:\s*"(edgion_[a-z0-9_]+)"', catalog)
    if len(catalog_names) != len(set(catalog_names)) or set(catalog_names) != inv:
        print("METRIC CATALOG MUST MATCH INVENTORY EXACTLY WITHOUT DUPLICATE FAMILIES")
        print("  missing:", sorted(inv - set(catalog_names)))
        print("  extra:", sorted(set(catalog_names) - inv))
        return 1
    declared_classes = dict(re.findall(r'Definition\s*\{\s*name:\s*"(edgion_[a-z0-9_]+)"\s*,\s*kind:\s*MetricType::\w+\s*,\s*class:\s*MetricClass::(\w+)', catalog))
    inventory_classes = {}
    for row in INVENTORY.read_text().splitlines():
        if re.match(r'\|\s*edgion_', row):
            columns = row.split('|')
            inventory_classes[columns[1].strip()] = columns[6].strip()
    if {name: value.lower() for name, value in declared_classes.items()} != inventory_classes:
        print("METRIC INVENTORY LIFETIME CLASSES MUST MATCH THE CANONICAL CATALOG")
        return 1
    bypass = []
    for source in production_sources(REPO):
        relative = str(source.relative_to(REPO))
        if relative not in RAW_BACKEND_FILES and raw_interface_bypass(source.read_text()):
            bypass.append(relative)
    if bypass:
        print("RAW METRIC REGISTRATION BYPASSES CANONICAL CLASS INTERFACE:", *bypass, sep="\n  ")
        return 1
    danger = scan_dangerous_labels([REPO / s for s in SCANNED])
    if danger:
        print("HIGH-CARDINALITY LABELS (allowlist in LABEL_ALLOWLIST with a reason, or remove):")
        for name, key in danger:
            print("  ! %s uses label '%s'" % (name, key))
        return 1
    print("OK: %d metrics, inventory in sync." % len(code))
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        guard_self_test()
    sys.exit(main())
