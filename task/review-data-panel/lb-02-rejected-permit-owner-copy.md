# Evaluate borrowed backend ownership for rejected circuit-breaker admission

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `3ae7c12a2`.

## Evidence and potential benefit

`routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer` clones the selected
attempt's `lb_backend` Arc before clearing `ctx.cb_permit` and calling
`backends/circuit_breaker/mod.rs::CbPermit::try_acquire`. That helper consumes
the Arc. Successful admission moves it into the RAII permit; rejection drops it.
The copy allocates no payload and does not deep-clone the backend.

A borrowed `&Arc<LbBackend>` admission API could clone only after successful
admission. It would eliminate one Arc increment/decrement on each rejected
attempt. Successful permits still need an independent backend owner. No measured
saturated-load benefit establishes whether this change is worthwhile.

The existing `get_current_upstream(&self)` accessor lends through the whole
context, while admission clears a separate context field before attempting the
new reservation. A direct replacement with a borrowed backend therefore needs
its borrow scopes resolved. A narrow field projection or controlled reborrow
may work; adding a new accessor is an option, not a demonstrated requirement.
This is deferred for value and interface review rather than reported as a defect.

## Decision and candidate constraints

Choose whether to retain the current owned API or measure a borrowed-input
candidate under realistic accepted/rejected ratios and same-endpoint contention.
Compare caller and test complexity against the denial-only atomic saving.
Do not add a parallel reservation protocol or new per-request storage.

Preserve the current selected-endpoint branch and release-before-acquire order.
`begin_upstream_attempt` already releases the prior permit before live lookup;
the later admission-site clear is also preserved unless independently justified.
Do not clear a permit earlier on a formerly skipped branch merely to satisfy
the borrow checker. Do not take the attempt's `lb_backend`: other LB accounting
still needs its original owner.

`LbBackend::try_acquire_cb_permit` performs one atomic conditional increment;
rejection must leave inflight unchanged. Successful guards must own their backend
and release exactly once on normal drop, early return, cancellation or unwind.
Keep endpoint identity and captured policy unchanged. DirectEndpoint and
ExternalJump without selected LB endpoints retain their bypass behavior.
Preserve the single shed metric and current HTTP/gRPC non-retrying failure
responses; this task authorizes no new observability or outlier ejection.

## Acceptance evidence

Review every API caller, including contention tests that currently transfer an
owned Arc into admission. Verify zero-cap and saturated rejection, successful
ownership, per-endpoint independence, drain/readmit, same-endpoint retries,
concurrent acquire/drop churn and release on early return/cancellation/unwind.
Compare before/after saturated and ordinary traffic performance before claiming
an operational gain. Keep ownership and counter correctness separate from timing.

For this documentation audit, five existing permit tests and 104 upstream-peer
tests passed serially. Formatting check, workspace check/Clippy and agent-doc
validation passed with existing unrelated warnings. These are baseline checks,
not verification of an unimplemented borrowed API or measured performance. No
runtime source, live integration scenario or dependency was changed.
