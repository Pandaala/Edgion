---
name: build-peer-tls-helper-dedup
description: Read when reviewing build_peer_from_lb_selection inlining upstream.tls construction instead of calling record_tls_to_upstream / LB paths keep a second hand-rolled copy of the TLS-metadata write (backends-F007).
knowledge_type: regression-guards
---

# backends-F007 — one upstream TLS metadata writer

The May 24, 2026 correction removed a duplicated inline `BackendTlsInfo` write
from `build_peer_from_lb_selection`. That historical implementation had four
native mode arms; those pseudo-kind arms have since been removed.

Current ordinary Service and controller-resolved ExternalName selection use
`build_peer_from_lb_selection`; self-resolved ExternalName uses
`build_external_name_peer_with_resolver`. Both call `record_tls_to_upstream`
before the canonical TLS peer builder. Its unconditional assignment clears old
TLS metadata when a later attempt is plaintext. Absent BackendTLSPolicy stays
plaintext even for Service port 443.

Preserve the shared writer when adding metadata or changing peer construction.
Inspect both real branches and TLS-to-plaintext tests; historical DRY acceptance
cannot suppress a new missing write, stale field, or policy-provenance defect.
