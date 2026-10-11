---
id: ARCH-393
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-513
analyst: /root/analyze_conversion
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Select the actual mirrored capture before asserting its POST body

`test_post_body_mirrored` in `edgion-tests/src/client/suites/edgion_plugins/request_mirror/request_mirror.rs:210–273` checks the method and body of `captures[0]`. Its polling helper at76–126 only establishes that some capture has `headers.x-mirror == "true"`; it returns the entire array. Those are different observations. The authored basic route sends both the primary request and its mirror to the same test-http Service and `/mirror/capture` endpoint. The real server appends both records under the same canonical trace ID in arrival order; it does not promise that the mirror is first.

A feasible false-pass witness is the primary POST arriving first with the correct `shadow-test` body, followed by a mirror POST with an empty or altered body. Polling succeeds because the second record is marked as a mirror, while the assertion reads the first primary record and passes. This is conditional source evidence, not a measured race or an incident. Conversely, no mirror at all still fails the poll. A separate committed-body suite correctly selects the marked mirror, and the existing mirror body/snapshot unit tests remain useful; they do not make this registered ordinary POST case observe the correct record.

Use the existing private `find_mirror_capture` in this case before reading method or body. Apply the same capture-identity correction to `test_path_and_query_preserved:400–437`, which also indexes the first record. Keep explicit failure if no marked capture exists and retain the current request, marker, polling retries, method/body assertions and all eleven registered cases. This requires no new registry, transport or production API. Changing server ordering is an unsuitable alternative because main and detached mirror arrival order is legitimate asynchronous behavior. Checking that any record has the expected body is also insufficient: the primary can satisfy that predicate.

Future verification should cover primary-first and mirror-first arrays, missing marked records, a valid primary plus wrong mirror body/method, and correct marked bodies. Preserve real local/Kubernetes routing, canonical correlation, committed snapshot isolation, request-only BodyStorage, streaming tee budgets, mirror cancellation and discarded responses. Do not change production copying or budgets to simplify the harness. Existing findings264/265/266/296/348 and382 concern other owners; this is capture identity, not response-read failure or path comparison. Full primary and deciding producer/caller/fixture seams were personally read; review remained read-only without executing tests.
