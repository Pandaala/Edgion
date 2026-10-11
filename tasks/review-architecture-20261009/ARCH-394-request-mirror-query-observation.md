---
id: ARCH-394
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-513
analyst: /root/analyze_conversion
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Observe the query before claiming RequestMirror preserves it

`test_query_string_forwarded` in `edgion-tests/src/client/suites/edgion_plugins/request_mirror/request_mirror.rs:662–713` sends `/mirror/capture?key=value&env=prod` and reports query forwarding after finding only `x-mirror:true`. It never compares any query value. The related `test_path_and_query_preserved:400–437` sends another query but checks only whether a recorded path contains `/mirror/capture`. The real `MirrorCapture` schema and `mirror_capture_handler` in `edgion-tests/src/server/test_server.rs:40–51,1041–1117` store `req.uri().path()`, which excludes the query. Their serialized observation cannot establish the property these cases claim.

A concrete conditional regression is building the mirrored URL from `base_url + path` while dropping the `session.get_query()` branch at `request_mirror/plugin.rs:313–319`. A marked mirror would still reach `/mirror/capture`, and both cases would pass their present predicates. Correcting capture identity as proposed by ARCH393 would not detect this loss: the selected mirror still has the same path and marker. No current production query-loss incident is claimed.

Add a narrow test-only observation of the received query or complete path-and-query to the mirror capture record, populate it before consuming the body, and require the selected marked capture to match the authored request target in both cases. Preserve the existing path field and other consumers; the slow capture handler constructs the same record and must supply the observation consistently. Reject a missing or malformed observation rather than substituting an empty value. Keep receipt, method/body, header, main-request and cancellation assertions; do not remove either case to erase its claimed coverage.

A mirror marker, a matching path or a trace ID proves delivery, not query preservation. Parsing a query out of the existing path cannot work because the producer has discarded it. A generic echo-parser or capture framework is unnecessary; this is one observation seam shared by the existing fixture and two assertions.

Future verification should make the cases fail when the query is omitted, reordered contrary to the exact authored target, or altered while a marked mirror still arrives, and pass for the unchanged target. Use the existing shared local/Kubernetes topology and preserve body capture limits, correlation, canonical transport, request-body storage/streaming ownership and mirror resource budgets. No production URL, credential, logging or metrics behavior changes are needed. Existing382 checks a different forward-path prefix predicate; this defect owns a missing mirror-query observation. Full primary and decisive current source/fixtures/original contracts were read without execution or mutation.
