# Native WAF hot reload

The wire matrix uses the existing transport backends and recorder. It covers
HTTP/1.1 and HTTP/2 on both sides, allowed bodies of 65,535 / 65,536 / 65,537
bytes, and a blocked marker inside the inspection window. A byte-count and
incremental SHA-256 report verify the allowed request arrives exactly once and
unchanged. The recorder verifies rejected requests expose no headers or body.
A chunked HTTP/1.1 request waits for local 100 Continue, pauses across an
observation window before completing a split marker, and must remain invisible
to the backend throughout. A marker after the first 64 KiB is allowed initially,
then rejected after the hot reload to explicit full inspection; invalid-rule
reload retains that full-inspection generation.
The next reload selects detection-only prefix inspection and proves a matching
body larger than the window is forwarded unchanged across the same protocol matrix.

This scenario proves initial publication, a valid generation swap, retention of
the last valid generation after a non-final WAF deny status, recovery with a
later valid generation, deletion fail-close behavior for the dangling ExtensionRef, and
re-creation. The initial phase also proves a phase-2 body denial before the
request reaches the backend. Its phase-3 rule sends a final 403 after the
backend responds; the wire test proves that the Gateway replaces the
backend's original 200 response.

A configured-prefix reload sets `prefixSize: 1m` and repeats the H1/H2 matrix
at the 1 MiB boundary, verifying that the new window takes effect before the
subsequent detection-only reload.

The request-control reload covers all four selected request-scoped `ctl`
actions. It checks that a tag exemption applies only to the matching request,
including successful and failed chains with staged captures and macro-based
setvars. A JSON processor override parses a `text/plain` body; the raw-body
switch is tested in both directions against the same phase-2 rules. A target
exclusion suppresses only the response-status child of a tagged phase-3 chain.
The generation then reloads to the recovery rules.
