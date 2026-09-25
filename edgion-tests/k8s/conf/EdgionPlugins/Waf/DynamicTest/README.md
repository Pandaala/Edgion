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
the last valid generation after a non-final WAF deny status, recovery with a later valid
generation, deletion fail-close behavior for the dangling ExtensionRef, and
re-creation. The initial phase also proves a phase-2 body denial before the
request reaches the backend.

A configured-prefix reload sets `prefixSize: 1m` and repeats the H1/H2 matrix
at the 1 MiB boundary, verifying that the new window takes effect before the
subsequent detection-only reload.

Each phase publishes digest-pinned `WafRuleBundle` resources through one
`WafPolicy` EdgionConfigData. The plugin keeps the same `policyRef` while the
policy selects the phase's bundle. An invalid policy references a missing bundle
and preserves the last valid generation. Deletion removes the policy and all
rule bundles as well as the plugin.
