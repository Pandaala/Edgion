---
name: edgionplugins-lkg-identity-coverage-not-standalone
description: Use when reviewing EdgionPlugins last-known-good identity integration coverage, reconnect full-snapshot plugin retention tests, or proposals to add a standalone D0031-style scenario before the cross-source identity contract exists.
status: wont-fix
finding_id: D0031
closed: 2026-09-05
knowledge_type: intentional-design
---

# EdgionPlugins LKG Identity Coverage Is Not a Standalone Defect — WONT-FIX

## Conclusion

Ordinary incremental EdgionPlugins updates are already correct after B0062: invalid
DSL/WAF `update` retains the last activatable generation, and a compressed same-key
`add` replaces unconditionally and fails closed. A standalone blocked integration
todo for reconnect identity coverage is not an independently actionable defect.

Security exception: invalid DSL/WAF updates do not retain a previous runtime
when either generation contains an executable request-stage cross-namespace
RequestMirror. Both delta and full-snapshot publication use the incoming sealed
fail-closed runtime, preventing stale mirror authorization. See
[mirror authorization](../../request-mirror/regression-guards/mirror-reference-grant-enforcement.md).
The ordinary retention statements below apply outside that exception; this does
not resolve D0030's separate resource-identity contract.

This closes D0031 and deletes
`tasks/todo/add-edgionplugins-lkg-identity-integration-coverage.md`. It does **not**
close D0030. Reconnect and relist full snapshots still key last-known-good retention
by `namespace/name` only; that remaining correctness gap and its later end-to-end
scenario belong on D0030.

Fix suggestions of the form "add the identity integration scenario now", "encode
current `replace_all` same-key retention as the desired reconnect contract", or
"keep a separate blocked D0031 todo" are **not accepted**.

## Core Rationale

**1. The live update path already distinguishes update from recreate.**

`PluginStore::apply_delta` removes first, installs `add` unconditionally, then
applies update-only last-known-good retention. Colocated tests prove an invalid
WAF update keeps the previous activatable Arc and an invalid same-key add
replaces it. Operators applying or recreating resources while Controller and
Gateway stay connected do not need a new integration scenario to make that path
true.

**2. The missing coverage is D0030's acceptance, not a second finding.**

`PluginStore::replace_all` still cannot tell an invalid same-object update from
a delete/recreate at the same key. Writing the D0031 scenario before that
cross-source identity contract exists would either fail against current
`replace_all` or lock the name-keyed reconnect behavior into the harness.
Existing integration tests stay frozen. After D0030 lands, add one new dynamic
scenario there: same-identity invalid update retains, new-identity invalid
recreation fails closed, reconnect/full-snapshot matches the equivalent partial
history, and a later valid generation recovers. Use readiness or `wait_phase`
polling, not fixed sleeps.

**3. A plugin-local Kubernetes UID shortcut is not a test substitute.**

The same reason D0030 stayed deferred applies to coverage: FileSystem and etcd
inputs plus conf-sync reconnect snapshots have no UID event history. A
Kubernetes-only fixture would guess one source's identity and leave reconnect
parity undefined for the others.

## Fix Suggestions Not Accepted

- Add the D0031 dynamic scenario before D0030 implements identity — the
  assertions have no implemented contract and would encode a guess.
- Treat current full-snapshot same-key retention as the desired reconnect
  contract — that is the D0030 defect, not accepted product behavior.
- Keep a separate blocked integration todo — it duplicates D0030 and was
  closed as not independently actionable.
- Edit existing EdgionPlugins integration assertions to manufacture this
  coverage — the campaign freeze and the original D0031 scope both forbid it.

## Re-evaluation Triggers

Re-open this decision only if:

- D0030 is implemented and still has no source-to-reconnect identity scenario.
- Incremental `apply_delta` no longer retains last-known-good on invalid
  DSL/WAF updates or no longer replaces on compressed same-key adds.
- A review claims ordinary in-session plugin updates are broken solely because
  this standalone todo is gone.

## Reference Cases

- Closed D0031 / `tasks/todo/add-edgionplugins-lkg-identity-integration-coverage.md`
- Remaining product finding: `tasks/pending/edgion-gateway-file-optimization/deferred/D0030-edgionplugins-last-known-good-resource-identity.md`
- Store contract: `edgion-gateway/src/plugins/http/plugin_store.rs` (`apply_delta`, `replace_all`)
- B0062: `tasks/pending/edgion-gateway-file-optimization/batches/B0062-http-plugin-snapshot-delta-semantics.md`

## Resolution references

- `D0031` — 2026-09-05 (wont-fix).
