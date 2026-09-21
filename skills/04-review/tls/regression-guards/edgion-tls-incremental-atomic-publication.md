---
name: edgion-tls-incremental-atomic-publication
description: Use when reviewing EdgionTls delta complexity, certificate/matcher coherence, last-known-good transitions, or collision logging.
knowledge_type: regression-guards
---

# EdgionTls publication updates individual host slots

## Conclusion and scope

The Gateway publishes resource entries, listener/hostname slots, exact attachment
candidates, and validity counts in one immutable TlsSnapshotData. The global
TlsStore and TlsCertMatcher share one ArcSwap; standalone stores are isolated.
Handshakes and the TLS section of /store-stats capture one snapshot for related reads.

Persistent rpds maps share unchanged entries, listener buckets, hostname slots,
and attachment buckets. Partial updates visit only changed resource keys and
their old/new hostname and attachment memberships, even when all resources share
one listener. They never clone the full resource table or rebuild a whole listener.

## Rationale and evidence

Canonical authority: skills/00-design-rules.md section S and
skills/01-architecture/02-gateway/11-conf-handler-guidelines.md section 3.1.

Before incremental-publication-03-edgion-tls, apply_partial_update cloned every
TlsEntry and rebuild_matcher_from_data traversed all certificates and hosts.
The matcher was published before the resource entries. The old handshake already
captured both matcher indexes consistently; there was no evidence that this
separate statistics/store window itself caused incorrect certificate selection.

The implementation is in tls/store/tls_store.rs and cert_matcher.rs. IndexDeltas
collects effective old/new memberships. TlsSnapshotData::apply_indexes copies
changed HAMT paths and reorders affected candidate slots. Shared canonical_key
and lookup helpers preserve Rfc6125HostnameMatcher normalization, exact-first lookup,
and single-label wildcard semantics. Other resource stores retain their representations.

## Preserved boundaries and limits

- TlsEntry.compiled.tls is the effective resource (TlsEntry.compiled also owns the
  cold-compiled handshake state published with it); validation records certificate validity;
  matchable additionally requires bounded attachment dimensions. Statistics count
  validation.is_valid, not matcher admission.
- Invalid updates can retain the old valid resource and set rejected_active.
  Rejected hosts/listeners never enter indexes. Repeated rejection shares the
  entry; a valid update clears the flag and rearms rejection logging.
- Add denotes a new generation and cannot inherit a deleted certificate.
  Overlapping direct-call batches retain sequential add, update, remove semantics:
  update may retain an add from that batch; remove wins finally.
- Full replacement removes absent keys and retains a predecessor only for
  invalid same-key input with valid attachment dimensions. Full rebuild is intentional.
- All host collision candidates remain available. Ordering is oldest
  creationTimestamp, then namespace, then name; None timestamps retain None-first.
  Warnings compare member keys and winner identity, not Arc identity. Renewal and
  unrelated updates do not repeat unchanged collision warnings.
- Exact attachments retain persistent member keys even above 64. Overflow rejects
  the entire group; deletion back to 64 reconstructs bounded ordered candidates.
  Overflow warnings rearm on recovery.
- Work still scales with candidates in each affected hostname slot; persistent
  paths are not constant-time. Exact attachment runtime scans remain bounded by
  64 candidates and each resource's bounded hosts.
- Production writers rely on the conf-sync CacheData write lock. A second writer
  needs explicit serialization or CAS. No worker or cache implementation is added.
- This is not cross-kind atomicity with Gateway listener publications. Separate
  snapshot captures can straddle an update and must not be combined for one decision.

## Verification and re-evaluation

### Final-resource attachment prewarming

Re-review finding CAND-P1-1 exposed a preexisting publication-order defect:
regular-expression SAN compilation clones EdgionTlsSpec, whose Clone resets
resolved_listener_attachment_index. Warming the input before compilation left
the final published resource unwarmed, even though the exact matcher indexed
its serialized attachment vector. The handshake's non-lazy identity recheck
then skipped the dual-client-auth conflict rejection required by
skills/01-architecture/02-gateway/04-tls/01-downstream-tls.md.

When the Gateway also had usable serving certificateRefs, this retained its
frontend policy but omitted the conflicting EdgionTls SAN policy. Gateway CA
verification was not bypassed. Without Gateway serving certificates, the later
empty certificate set still failed closed.

validated_entry now compiles SAN expressions before warming the final resource.
The handshake getter remains non-lazy. The regression test
published_regex_san_preserves_attachment_and_rejects_dual_client_auth publishes
a real certificate with a regex SAN through full, add, and update paths, then
passes the exact matcher result into the real overlay selector without manual
prewarming. Each phase must reject dual client-auth. This is in-process coverage,
not a cross-process TLS handshake test.

Fresh follow-up verification: cargo test -p edgion-gateway --lib tls:: passed
192 tests, including this regression. Independent review of the final fixture
and production ordering returned LGTM.

### Incremental-publication verification history

Earlier focused verification: cargo test -p edgion-gateway --lib tls::store passed
22 tests; the broader tls:: selection passed 191 tests; the shared Rfc6125HostnameMatcher module
passed 19 tests. Coverage includes unchanged
same-listener hostname sharing, unaffected listener/entry identity, wildcard
deletion, exact priority, retained snapshots, attachment/listener migration,
rejection/recovery, overlapping batches, collision transitions, 64/65 overflow
recovery, unrelated candidate isolation, and full/partial equivalence.

TlsLastKnownGood exercises valid -> invalid -> recovered -> invalid Secret
hot reload and restores a valid Secret on success or failure. Controller rejection
may prevent an invalid Secret from reaching the Gateway, so this scenario alone
does not prove the internal rejection branch; store unit tests do.
Format, workspace check, workspace Clippy, agent-doc validation, SSA-force,
metrics-inventory, Pingora-source-policy, Gateway no-tracing, and git diff checks
passed. Clippy has workspace warnings, including the retained Arc<Vec<...>>
candidate representation; this is not a warning-free result. Independent review
returned LGTM, including a second review of the dynamic fixture-serial barrier.
Cross-process execution is pending because another workspace occupies the shared
integration ports; its running binaries are not evidence for this implementation.

Re-evaluate on changes to event compression, writer ownership, hostname semantics,
attachment bounds, precedence, or snapshot consumers.

## Resolution references

- incremental-publication-03-edgion-tls — validated performance and publication
  consistency defect; implemented and independently reviewed, dynamic execution pending.
