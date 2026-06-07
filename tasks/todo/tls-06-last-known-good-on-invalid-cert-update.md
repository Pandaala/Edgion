# tls-06 — Preserve last-known-good cert on an invalid cert update (no listener drop)

**Promoted from review finding:** `tasks/review-envoy/tls-06-invalid-cert-update-drops-listener.md`
(deleted on promotion; this task now tracks the work).
**Severity:** critical (production availability)
**Status:** todo — needs a brainstorming/design pass before implementation (cross-plane, architectural).

## Confirmed defect

When an updated `EdgionTls` carries a certificate that fails validation (e.g. a
cert-manager renewal race pushing an empty `tls.crt`, or an operator mistake),
the gateway **drops the previously-serving certificate** instead of keeping the
last-known-good one. The affected host serves zero new TLS connections until a
valid cert is re-pushed. No rollback, no listener-scope isolation, no dedicated
alert distinguishing "bad push" from "intentional removal".

Code path (verified):
- `src/core/gateway/tls/store/tls_store.rs:154-173` — `apply_partial_update`
  **unconditionally overwrites** an existing valid `TlsEntry` with the new
  `TlsEntry { validation.is_valid = false }`.
- `tls_store.rs:198-205` — `rebuild_matcher_from_data` **skips** `!is_valid`
  entries, so the host disappears from the matcher entirely.
- `cert_matcher.rs:51-62` — `match_sni` then returns `None` → handshake fails
  with `cert not found`.

Result is worse than "serves the bad cert": **neither the new (bad) nor the old
(good) cert is served — the host goes fully dark.**

Controller side is NOT the cause: `EdgionTlsHandler::validate`
(`handlers/edgion_tls.rs:48-88`) only emits a warning + `Accepted:False` status
condition (via `check_key_matches_cert`); `parse()` still returns
`ProcessResult::Continue`, so the bad cert is synced to the gateway. The harm
happens at the gateway store.

## Envoy baseline (the proven design)

xDS ACK/NACK is the model: a bad resource push is NACKed (error reported to the
control plane) and **rejected**, while the data plane **keeps serving the last
successfully applied config**. NACK is not cosmetic — the bad resource is not
applied. SDS inherits this; the granularity is **per-resource (per-secret)**,
not per-host.

### Cold-start boundary (out of scope, matches Envoy)

last-known-good is a *process-lifetime* protection, not a persistent one. If the
control plane and data plane both restart and the control plane's current
authoritative state is bad (no prior good cert exists in this process), there is
no last-known-good to fall back to — the host has no cert. Envoy has the same
limitation (it does not persist SDS secrets across restarts by default).
**This is acceptable and explicitly out of scope** — do not build persistent
cert caching to "fix" cold start.

## Design options (decide in brainstorming)

These are two layers, not either/or. Most robust = both.

### Layer 1 — Gateway store: per-key last-known-good (robust last line)
- In `apply_partial_update` (add/update) and `replace_all_tls`: when a cert
  fails `validate_cert` and an existing **valid** entry under the same key
  (`namespace/name`) is present, **retain the old entry, discard the bad update**
  instead of overwriting. Emit `tracing::error!` (config-sync path only, not the
  handshake path) + a counter.
- No new cache needed: `tls_data: ArcSwap<HashMap<String, TlsEntry>>` already
  holds the last-known-good keyed by `namespace/name` (matches Envoy's
  per-resource NACK granularity). Matcher rebuild logic is untouched.
- Pros: protects regardless of how the bad cert reached the store (version skew
  during rolling upgrade, future direct-config paths, validation drift). The
  data plane never self-harms — this is the indispensable layer.
- Cons: invalid update no longer lands in the store, so
  `get_invalid_certs`/`get_cert_stats`/`get_validation_status` no longer reflect
  the rejected push — visibility must move to the counter/log.
- Estimated size: moderate (a few lines in two functions of one file + tests).

### Layer 2 — Controller: gate before sync via existing `ProcessResult::Skip`
- `ProcessResult::Skip` (`handler.rs:12-17`) already means "do not write to
  ServerCache and do not delete the existing entry" (`processor.rs:650-653`).
  So returning `Skip` on an invalid cert **keeps the previous valid version in
  the cache and keeps syncing it** — the gateway never receives the bad update.
  No new retain-previous machinery required.
- Requires elevating controller validation from `check_key_matches_cert`
  (warning only) to the **full `validate_cert`** in `parse()` (secret is already
  resolved into `spec.secret` at `edgion_tls.rs:133` before this point), and
  returning `Skip` on failure.
- Pros: bad certs never sync (no wasted gateway work); error surfaces in K8s
  status (`Accepted:False`) which is the natural operator-facing place; matches
  the xDS "control plane owns validation + reporting" model.
- Cons: cannot stand alone — leaves the gateway store bug reachable via
  version skew / future non-controller paths. Behavior change: invalid cert now
  also means "not synced" (status already says `Accepted:False`, so arguably
  more consistent). Latent controller↔gateway validation-parity coupling
  (today both call the same shared `validate_cert`, so parity is exact).

### Recommendation captured during triage
Do both, but **Layer 1 (gateway store) is non-negotiable** — it is the actual
serving point and must not drop a serving cert no matter the source. Layer 2 is
a valuable, elegant complement (cleaner status story + less wasted sync).

## Open questions for brainstorming
- Counter shape: `edgion_tls_cert_update_rejected_total{namespace, name, reason}`
  (issue-suggested) — confirm label cardinality is bounded and aligns with the
  project's metrics-explosion-prevention rules in `skills/04-review/observability/`.
- Do we keep the gateway-side `validate_cert` as the authority (Layer 1) and
  treat Layer 2 as defense forward, or invert? Validation-parity policy.
- `replace_all_tls` (full resync) has no natural "previous" — confirm reading
  the current store to carry forward last-known-good is correct and does not
  mask an intentional removal (removal flows through the `remove` set / delete,
  not an invalid update, so it should be safe).
- Status/observability: should the gateway emit a distinct signal so operators
  can alert on "cert push rejected, serving stale-but-valid" independently of
  generic handshake errors?

## Acceptance criteria (sketch)
- A valid→invalid cert update on a serving host does NOT drop the host; the
  last-known-good cert keeps serving new connections.
- An intentional removal still removes the cert.
- A brand-new EdgionTls with an invalid cert is simply not served (no regression).
- Cold start with a currently-bad cert serves nothing for that host (accepted,
  documented, matches Envoy).
- Operators get a distinct rejection signal (counter and/or log + K8s status).
- Integration test covering the valid→invalid→serving-old-cert scenario per
  `skills/05-testing/01-integration-testing.md` (hot-reload/dynamic scenario).
