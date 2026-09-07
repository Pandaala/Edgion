---
name: negative-tests-need-out-of-band-observation
description: Historical review evidence for Tests / pingora fork terminal EOS regression suite; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Negative tests need out of band observation

## Recorded conclusion

Fixed: `aborted_response_never_dispatches_a_terminal_callback` could not fail. It read the callback count off the client body of an exchange it forces to fail, where a correct skip and an accidental dispatch both leave nothing readable (`unwrap_or_default()` turned the expected body error into `""`), and it accepted any non-timeout failure, so a 502 from an unreached origin passed too. The dispatch is now counted out of band via an `x-eos-probe` header, the aborting origin signals that it accepted the request, and an unexpected 502 fails. Mutation-verified.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [testing/negative-tests-need-out-of-band-observation.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/testing/negative-tests-need-out-of-band-observation.md)

## Resolution references

- `issue-abort-terminal-eos-test-false-positive` — 2026-08-24.
