# Correct ExtProc Phase Exchange Knowledge

## Meta

- Status: pending
- Priority: P2
- Category: documentation accuracy

## Evidence

At `b297af6e`, these knowledge pages still cite the removed
`ExtProcClient::exchange_all` implementation:

- `skills/04-review/ext-proc/regression-guards/ext-proc-phase-verdict-matching.md`
- `skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md`

The phase-matching page also describes collecting a fully valid multi-phase exchange
before applying any mutation. The current `AttachedExtProc` / `ExtProcStream` model
validates and applies individual phases; request-header mutation precedes the body
exchange. The historical prose must not be used as proof of current whole-exchange
rollback or transaction guarantees.

## Acceptance

Read applicable rules and trace the current attached request/response lifecycle, then
correct both owning pages and any affected index summary. Preserve valid phase-verdict
matching and interactive replacement requirements while clearly separating historical
implementation details from current guarantees. Do not silently weaken a requirement
because current code differs; record any unresolved semantic conflict separately.

No runtime redesign or integration-test change is requested. Obtain independent review,
run documentation validation and applicable project guards, and record any genuinely
needed new integration coverage as a separate task for later.
