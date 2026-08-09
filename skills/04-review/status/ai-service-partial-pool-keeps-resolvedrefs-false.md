---
name: ai-service-partial-pool-keeps-resolvedrefs-false
description: Use when reviewing EdgionAIService credential-pool status semantics for partially resolved Secret references.
---

# EdgionAIService partial pools keep ResolvedRefs false

## Decision

`EdgionAIService.status.conditions[ResolvedRefs]` describes every configured credential reference,
not whether the service has enough runtime material to remain active. It is `True` only when all
credential references resolve successfully.

When at least one credential resolves and another is missing, denied by ReferenceGrant, or invalid:

- `ResolvedRefs=False` uses the shared deterministic reason priority;
- `PartiallyInvalid=True/InvalidCredentials` lists bounded aliases and fixed error reasons;
- the usable credential subset remains serialized to the Gateway and the service stays active.

When no credentials resolve, `ResolvedRefs=False` remains the primary reference signal and the
service is inactive. `PartiallyInvalid` is false because no partial-good subset exists.

## Why both conditions are necessary

`ResolvedRefs` answers whether every declared object reference is valid. Reporting it as true when
one credential is omitted hides ReferenceGrant denial and makes operator intent appear fully
realized. `PartiallyInvalid` answers the separate runtime question: whether a valid subset is still
serving despite omitted entries.

This deliberately allows `ResolvedRefs=False` and `PartiallyInvalid=True` to coexist. The former is
the loud reference failure; the latter explains that availability is degraded rather than absent.

## Rejected alternatives

- Set `ResolvedRefs=True` whenever `resolved_count > 0`: this conflates partial availability with
  complete reference resolution.
- Special-case only `RefNotPermitted`: missing and malformed Secret references are unresolved too,
  and the shared condition contract already models their reason priority.
- Disable the whole service after one credential fails: credential pools are intentionally
  partial-good and retain usable siblings.
