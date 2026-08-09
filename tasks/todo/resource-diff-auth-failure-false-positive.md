# Resource diff treats Controller authentication failure as synchronization success

**Status**: todo
**Priority**: P2 — integration synchronization verification can produce a false positive
**Filed**: 2026-07-29, found while debugging the JSON Schema validation hot-reload scenario

## The defect

The integration `resource_diff` check can receive HTTP 401 while fetching
Controller resources, report the fetch failures as warnings, and still finish
with a successful synchronization result.

This makes the startup barrier weaker than its name implies: Gateway and
Controller resource equality has not been proved when one side could not be
read.

## Required behavior

- Authentication, transport, decoding, or non-success HTTP errors from either
  resource source must fail the resource-diff command.
- An empty resource set is valid only after a successful fetch that explicitly
  returned an empty set.
- Add an integration or command-level regression test for a wrong Controller
  token and assert a non-zero exit.
- Keep the normal authenticated resource-diff startup barrier passing.

## Scope note

This did not cause the JSON Schema dynamic-test failure. That failure was a
test listener mismatch, and the Controller/Gateway logs independently proved
the relevant resources were synchronized. This task records the unrelated
verification weakness without expanding the plugin feature change.
