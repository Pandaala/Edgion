---
id: ARCH-390
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-510
analyst: /root/analyze_outbound
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Distinguish the final RegionRoute jump from the preceding failover message

The registered `my_region_failover_to_nonlocal` case at `region_route.rs:937–998` claims to prove that selecting europe follows one failover hop to ohio and performs an external jump. Its two purported routing checks are dependent: `has_redirect` requires `failover ` and `europe→ohio`, while `has_jump` merely requires `→ohio`. The first successful check necessarily makes the second true, even when no separate jump record exists. HTTP 500 or 502 alone does not identify the selected upstream.

Both current fixture trees configure `myRegion: virginia`, `europe.failoverTo: ohio`, and ohio's endpoint as `127.0.0.1:30002`; ohio itself redirects to virginia only when selected directly. Production `apply_failover_redirect` emits `failover europe→ohio` before `route_to` separately emits `→ohio 127.0.0.1:30002 sni=...` and attempts to install the jump. The latter also precedes the setter outcome, so even a full jump-message check needs the existing terminal security evidence. The canonical external-peer path records `BackendLocalhostNotAllowed` when its resolved loopback address is rejected.

A concrete discrimination failure is selecting port 30003 instead of the authored ohio port 30002 while retaining the correct region label and redirect. Both loopback destinations trigger the same canonical security rejection and accepted HTTP status; the current test passes for either. An observation containing only the redirect and an unrelated accepted 500/502 also satisfies its routing predicates. These are conditional false-PASS paths in the assertion, not evidence that current production routing is broken or that an incident occurred.

Keep the correction local to this existing case. From its trace-correlated access log, require the expected route and the actual RegionRoute execution, retain the redirect assertion, and separately require the full selected-target fragment `→ohio 127.0.0.1:30002`. Require the existing `error` diagnostic `BackendLocalhostNotAllowed`; missing observations fail. The existing `RegionRouteValidation::check_traffic` already scopes canonical plugin records and distinguishes a selected endpoint from the loopback rejection, demonstrating a feasible repository-owned approach. No new request logging, metric, production API or general assertion framework is needed.

Keep the accepted 500/502 envelope, host/header inputs, trace isolation, retry and wrapper deadlines, local/Kubernetes fixture ownership and test identity. Preserve one-hop failover, base/overlay semantics, raw-path forwarding and the loopback guard. Replacing loopback with successful remote delivery would change this test's purpose and topology. Unit tests already cover one-hop selection and accepted/rejected jump outcomes; they do not replace the integration observation.

Future focused verification should pass the current exact endpoint plus redirect and typed rejection; wrong-port, redirect-only, missing-diagnostic and unrelated-error observations must fail. This repository-only review read the complete primary and actual relevant producer, unit and integration assertion bodies; it ran no tests, builds or network operations.
