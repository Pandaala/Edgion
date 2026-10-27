#!/usr/bin/env python3
"""Guard Cookie tokenization and reviewed main-header mutation boundaries.

This lexical guard detects multiline direct writes and header replacement. It
cannot prove aliases; the mutation ledger and real callback tests remain gates.
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "edgion-gateway/src"
MUTATION = re.compile(
    r"\.\s*req_header_mut\s*\(\s*\)\s*\.\s*(?:insert_header|append_header|remove_header)\s*\([^;]+;"
    r"|\*\s*(?:(?:[A-Za-z_]\w*\s*\.\s*)+req_header_mut\s*\(\s*\)|request)\s*=\s*[^;]+;"
    r"|\breq_header\s*\.\s*remove_header\s*\([^;]+;"
)
ALLOWED = {
    "ctx.rs": {"*request = candidate;"},  # Query-only validated projection.
    "ai/context.rs": {"*request = header;"},  # Entire request baseline restoration.
    "plugins/runtime/session_adapter.rs": {
        ".req_header_mut().remove_header(&http::header::EXPECT);",
        ".req_header_mut() .append_header(name.to_string(), value) .map_err(|e| Box::new(e) as PluginSessionError)?;",
        ".req_header_mut() .insert_header(name.to_string(), value.to_string()) .map_err(|e| Box::new(e) as PluginSessionError)?;",
        ".req_header_mut() .insert_header(name.to_string(), value) .map_err(|e| Box::new(e) as PluginSessionError)?;",
        ".req_header_mut().remove_header(name);",
        "*self.inner.req_header_mut() = candidate;",
        '.req_header_mut() .insert_header("Host", host_value) .map_err(|e| Box::new(e) as PluginSessionError)?;',
        "req_header.remove_header(&rule.name);",  # Dye anti-spoof removal.
    },
}
SPLIT = re.compile(r"\.\s*split\s*\(\s*';'\s*\)")
# Content-Type parsing and response Set-Cookie are separate grammars.
NON_COOKIE_SPLITS = {
    "ai/guards/request/evaluator.rs",
    "ai/guards/semantic/protocol.rs",
    "link_sys/providers/webhook/resolver.rs",
    "plugins/http/grpc_json_transcoder/response.rs",
}
for fixture in [
    '*session.req_header_mut() = next;',
    '*other\n .req_header_mut()\n=next;',
    'session.req_header_mut()\n .insert_header(name,value);',
    'req_header\n .remove_header(name);',
]:
    assert MUTATION.search(fixture), fixture
assert SPLIT.search("value\n.split\n( ';' )")
errors = []
seen = set()
for path in SOURCE.rglob("*.rs"):
    relative = path.relative_to(SOURCE).as_posix()
    text = "\n".join("" if line.lstrip().startswith("//") else line for line in path.read_text().splitlines())
    for match in MUTATION.finditer(text):
        statement = " ".join(match.group().split())
        if statement not in ALLOWED.get(relative, set()):
            errors.append(f"{relative}: unreviewed main-header mutation: {statement}")
        else:
            seen.add((relative, statement))
    if relative != "request_cookie.rs" and relative not in NON_COOKIE_SPLITS and SPLIT.search(text):
        errors.append(f"{relative}: unreviewed semicolon parser; use canonical Cookie owner or document distinct grammar")
    if "cookie_value_ref" in text or "plan_cookie_strip(" in text:
        errors.append(f"{relative}: retired Cookie parser")
for relative, statements in ALLOWED.items():
    for statement in statements:
        if (relative, statement) not in seen:
            errors.append(f"{relative}: stale mutation allowance: {statement}")
if errors:
    raise SystemExit("\n".join(errors))
print("Request Cookie ownership guard passed")
