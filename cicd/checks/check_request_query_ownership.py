#!/usr/bin/env python3
"""Guard direct main-request Query/target reads; semantic regression tests remain required."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "edgion-gateway/src"
# Each direct transport read has one narrowly owned reason. Detached outbound
# URL parsers and TLS path normalization use different objects and grammars.
ALLOWED = {
    "ctx.rs": {"let current = request.raw_path();", "match raw_target_authority(request.raw_path()) {",
        "let mut candidate = request.clone();", "*request = candidate;", "request.set_raw_path(&target)?;",
        "request.set_uri(updated);", "RawTargetAuthority::None => request.set_uri(replacement),"},
    "routes/http/proxy_http/pg_request_filter.rs": {"let raw_query = req_header.uri.query().map(str::to_string);"},
    "plugins/runtime/session_adapter.rs": {".replace_raw(candidate.uri.query().map(str::to_owned))?",
        ".replace_raw(candidate.uri.query().map(str::to_owned))?;",
        "let mut candidate = self.inner.req_header().clone();", "*self.inner.req_header_mut() = candidate;",
        "self.inner.req_header_mut().set_uri(uri);"},
    "ai/context.rs": {"*request = header;"},
    "plugins/http/grpc_json_transcoder/transport.rs": {"|| request.uri.path_and_query().map(|p| p.as_str()) != Some(plan.binding.rpc_path.as_str())"},
    "routes/http/redirect_http.rs": {"let path_and_query = req_header.uri.path_and_query().map(|pq| pq.as_str()).unwrap_or(\"/\");"},
}
PATTERN = re.compile(
    r"\.uri\s*\.\s*(?:query|path_and_query)\s*\(|\.raw_path\s*\("
    r"|\.\s*(?:set_uri|set_raw_path)\s*\(|\.\s*req_header\s*\(\s*\)\s*\.\s*clone\s*\("
    r"|\brequest\s*\.\s*clone\s*\(|\*\s*(?:(?:[A-Za-z_]\w*\s*\.\s*)+req_header_mut\s*\(\s*\)|request)\s*="
)
# Guard its own important formatting-sensitive patterns without filesystem edits.
for rejected in [
    "*session.req_header_mut() = candidate;",
    "*other_session\n .req_header_mut()\n = candidate;",
    "*self.inner\n .req_header_mut() = candidate;",
]:
    assert PATTERN.search(rejected), "whole-header replacement must be detected"

errors = []
seen = set()
for path in SOURCE.rglob("*.rs"):
    relative = path.relative_to(SOURCE).as_posix()
    raw = path.read_text()
    # Preserve offsets while ignoring whole-line comments. Cross-line expressions
    # are scanned together, so inserting a newline cannot bypass the guard.
    text = "".join(" " * len(line.rstrip("\n")) + "\n" if line.lstrip().startswith("//") else line
                   for line in raw.splitlines(keepends=True))
    for match in PATTERN.finditer(text):
        number = text.count("\n", 0, match.start()) + 1
        start = text.rfind("\n", 0, match.start()) + 1
        end = text.find("\n", match.end())
        statement = " ".join(text[start:end if end >= 0 else len(text)].split())
        if statement not in ALLOWED.get(relative, set()):
            errors.append(f"{relative}:{number}: unreviewed direct target access: {statement}")
        else:
            seen.add((relative, statement))
    for number, line in enumerate(text.splitlines(), 1):
        if "entry_raw_query" in line or "cached_query(" in line:
            errors.append(f"{relative}:{number}: retired query owner")
for relative, lines in ALLOWED.items():
    for line in lines:
        if (relative, line) not in seen:
            errors.append(f"{relative}: stale target-read allowlist")
if errors:
    raise SystemExit("\n".join(errors))
print("Request Query ownership guard passed")
