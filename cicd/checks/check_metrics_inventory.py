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
    "edgion-controller/src/conf_mgr/sync_runtime/metrics.rs",
]

CONST_RE = re.compile(r"const\s+\w+\s*:\s*&\s*(?:'[a-z_]+\s+)?str\s*=\s*\"(edgion_[a-z0-9_]+)\"")
MACRO_LITERAL_RE = re.compile(
    r'metrics::(?:counter|gauge|histogram)!\(\s*"(edgion_[a-z0-9_]+)"'
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
MACRO_CALL_RE = re.compile(r'(?:metrics::)?(?:counter|gauge|histogram)!\(')
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
    danger = scan_dangerous_labels([REPO / s for s in SCANNED])
    if danger:
        print("HIGH-CARDINALITY LABELS (allowlist in LABEL_ALLOWLIST with a reason, or remove):")
        for name, key in danger:
            print("  ! %s uses label '%s'" % (name, key))
        return 1
    print("OK: %d metrics, inventory in sync." % len(code))
    return 0


if __name__ == "__main__":
    sys.exit(main())
