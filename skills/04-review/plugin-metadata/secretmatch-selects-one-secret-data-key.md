---
name: secretmatch-selects-one-secret-data-key
description: Use when reviewing SecretMatch resolution or schema; secretKey selects exactly one Secret data entry and unrelated values must never become alternate credentials.
status: fixed
finding_id: cl-04
closed: 2026-08-24
---

# SecretMatch Selects One Secret Data Key

## Conclusion

`secretMatch` has two distinct key concepts: `key` reads the request value,
while required `secretKey` selects exactly one entry from `secretRef`. The
Controller resolves only `Secret.data[secretKey]`; it never treats all UTF-8
values in a Secret as an OR credential set.

The selected value must exist, decode as UTF-8, and contain non-whitespace
content. Failure clears stale `resolvedValues`, reports `ResolvedRefs=False`,
and composes with the consumer-specific unresolved policy. Secret values never
appear in logs, status, metrics, or descriptors.

## Why

Kubernetes Secrets commonly contain multiple fields. Treating every field as
an accepted token lets unrelated usernames, metadata, certificates, or other
values satisfy a security predicate. Requiring a selected data key makes the
credential contract explicit without forbidding multi-key Secrets.

Rejecting all multi-key Secrets and relying on a fixed conventional key were
not selected: the first makes unrelated Secret maintenance break policy, and
the second hides the contract in an implicit naming convention.

## Reference

- `cl-04` (2026-08-24)
- [unresolved-condition-references-need-consumer-policy.md](unresolved-condition-references-need-consumer-policy.md)
