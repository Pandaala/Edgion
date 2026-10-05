# B0083 — DNS parsing, transport tests and egress review

## Coverage and changes

All seven src/dns files, seven corresponding original unit-test files, src/egress.rs
and tests/unit/egress_test.rs were fully reviewed. Parent and independent reviewers
cert_review/watch_review traced resolver precedence, Controller background and
Gateway cache consumers, literals, returned-address classification, fallback,
64 KiB transport cap, 256-address cap, deduplication, timeouts and cache ownership.

HTTPDNS dotted paths now escape JSON Pointer metacharacters per segment; the
regression includes a literal key and array indexing. Parsing borrows candidates
instead of first collecting every candidate into an owned String. JSON arrays
retain untrimmed values; split forms trim; invalid items, errors and first-seen
order are preserved. No measured throughput claim is made.

Three HTTP transport tests moved from tests/unit/dns/http_fetcher_test.rs to
Common integration target tests/httpdns_fetcher.rs, preserving function names.
The private artificial cap setter is removed; the actual 64 KiB cap is tested.
No production visibility changes and no frozen edgion-tests edits.

## Decisions, review and validation

Both independent reviewers returned LGTM on the complete final five-path diff.
HTTPDNS parser 28 passed; new integration target 3 passed. Workspace check and Clippy passed on the final code. Format, agent docs, SSA, metrics inventory, Pingora source policy and transform
contract guards passed. Unit layout and Gateway tracing guards passed.
All 3,772 frozen integration fingerprints remain unchanged. Other scoped files need no local change, except the
known design work D0019 (prepared runtime/client audit) and D0020 (endpoint egress
policy), recorded in tasks/todo. Neither is claimed fixed by parser changes.
