---
id: ARCH-048
status: candidate
kind: architecture-debt
priority: P3
partition: PART-187
analyst: /root/analyze_outbound
independent_verification: confirmed by /root/verify_ip_union
---

# Remove transient heap strings from the canonical query percent decoder

uri_utils::url_decode collects up to two Unicode scalar values into a fresh
String after each percent sign, then drops that token after decoding or
preserving it. Nonempty tokens require heap backing although their UTF-8
representation is at most eight bytes. A value with 64 ordinary two-digit
percent escapes constructs 64 short token Strings in addition to the actual
output and byte-run storage. This is source allocation evidence, not an
executed allocator count or measured latency claim.

Actual consumers include EdgionHttpContext::cached_query through
parse_query_string, HTTP query matching, PluginSessionAdapter::get_query_param,
consistent-hash extract_hash_key for encoded argument names, and authentication
strip_query_param for encoded credential names. Caching avoids subsequent query
parses, not this first-parse cost; the latter two consumers decode directly.
ARCH-004 concerns immutable query-cache identity and has a different cause.

Keep the fix in this canonical decoder and preserve its exact current behavior.
A bounded stack UTF-8 buffer can collect the same up-to-two scalar values,
retain the existing token byte-length == 2 gate, call the same
u8::from_str_radix(..., 16), and append a failed token literally after flushing
the pending byte run. Two scalars require at most eight UTF-8 bytes, so no owned
token String or new dependency is needed. Keep the output String, run buffer
and public API. An equivalent scalar-slot implementation is possible if it
preserves both token consumption and the integer parser's accepted syntax.

Do not replace that parse with an ASCII-two-hex-digit-only check. The current
unsigned integer parser accepts a leading plus: %+A decodes to byte 0x0A and
%+0 to byte zero. Preserve this accepted behavior within the allocation-only
change. A plus consumed inside a failed token, such as %+G, remains literal;
an ordinary plus means space. The decoder consumes up to two scalars after a
percent sign even when the token is malformed or non-ASCII. Adjacent successful
tokens form one UTF-8 run; a run containing invalid UTF-8 becomes bytewise
Latin-1 characters in its entirety. Preserve short/non-ASCII malformed tokens,
flush boundaries, empty-pair skipping, no-equals empty values and last-value-wins
decoded duplicate keys. A naive byte loop or lossy external decoder can change
these contracts.

Original form-decoding and LB/auth decisions require decoded key comparison,
raw LB values and raw surviving authentication pairs. Retain the LB plain-name
fast path, frozen single-key query view and query-only credential mutation.
The bounded DSL builtin has its own byte limits, token handling and
malformed-UTF8 policy and remains outside this change. There is no need for
another decoder, new cache, Cow API or whole-query rewrite. Retain canonical
request/access-log ownership and redaction; add no traffic logs or metrics.

Existing URI tests cover ordinary plus, literal encoded plus, adjacent UTF-8
and basic malformed escapes. Future verification should also cover accepted
leading-plus tokens, failed consumed-plus tokens, short/non-ASCII malformed
tokens, invalid UTF-8 runs and decoded duplicate keys. Inspect that no owned
percent-token allocation remains and retain route/plugin/LB/auth wiring. Use
existing private test access; do not widen visibility or add runtime frameworks.
Original source, actual tests and scoped consumer/knowledge evidence were
personally read for independent verification. No builds, tests, network,
application execution or allocator benchmark were performed. Full primary
analysis receipts belong to REPORT-PART-187.json; independent verification
receives no source primary coverage.
