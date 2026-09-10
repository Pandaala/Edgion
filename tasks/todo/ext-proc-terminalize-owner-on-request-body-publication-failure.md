# Terminalize ExtProc Owners on Request Body Publication Failure

## Meta

- Status: pending
- Priority: P2
- Category: lifecycle correctness and diagnostics

## Evidence and impact

At `9b787978`, `edgion-gateway/src/plugins/http/ext_proc/plugin.rs::apply_request_common_response`
handles `overwrite_request_body` failure through `fail_mode`, ignoring the supplied
`Some(owner)`. Nearby malformed-verdict handling uses `fail_request_owner`, which
terminalizes the attached owner.

- A final successful wire exchange may already record success/normal-eos. A later local body publication failure can retain that misleading attached summary.
- If response phases remain, fail-open publication failure can leave the owner open, allowing the response companion to continue processing it.
- Existing `request_body_mutation_failure_honors_fail_open` and `request_body_mutation_failure_honors_fail_close` tests use `owner=None`, so they do not verify these lifecycle outcomes.

This is independent of immutable override-list sharing. Do not combine it with an
allocation-only refactor or alter the configured failure-policy vocabulary.

## Acceptance and verification

Trace the current owner and response-companion lifecycle, then make the smallest
consistent owner-terminalization fix. Cover `Some(owner)` for both failure policies,
including final-exchange and pending-response cases. Verify the terminal failure
summary, response-companion deactivation, unchanged original BodyStorage version and
unchanged generic failure response. Keep summary diagnostics in canonical access logs;
do not add parallel request logging or leak body/dependency details.

Use deterministic nonnetwork unit tests and independent review. Run the applicable
project checks. Do not modify existing integration tests; record any needed new
end-to-end coverage as a separate task for later implementation.
