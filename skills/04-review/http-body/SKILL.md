---
name: review-http-body
description: Review knowledge for http-body, grouped by decision type.
---

# http-body review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Explicit Request-Body Prebuffering for Late Reads](mandatory-design/explicit-request-body-prebuffer-for-late-read.md)
- [BF-01 — Request-Body Policy Uses an Atomic EGC Snapshot](mandatory-design/request-body-policy-egc-hot-reload.md)

## Intentional design

- [Request-body error diagnostics have one runtime classifier](intentional-design/request-body-error-diagnostic-classification.md)
- [Request-body read contracts belong to the body subsystem](intentional-design/request-body-read-contract-ownership.md)

- [Plugin body filter is read-only by design (bandwidth-throttle only) — DESIGN-TRADEOFF](intentional-design/body-filter-read-only-by-design.md)
- [Plugin-authored request framing is a trusted configuration choice](intentional-design/plugin-authored-request-framing-is-trusted.md)
- [HTTP/1 close after a response-stage local reply is a feature gap](intentional-design/response-local-reply-h1-close-is-feature.md)

## Regression guards

- [REST-to-GraphQL request adaptation](regression-guards/rest-to-graphql-request-adaptation.md)

- [Streamed request cap, trailer rejection, and ordered mirror publication](regression-guards/streamed-request-body-admission.md)

- [WAF prefix admission before backend dispatch](regression-guards/waf-prefix-before-dispatch.md)

- [The silent admission disposition requires unanimous facts — FIXED](regression-guards/body-admission-reads-pre-plugin-snapshots.md)
- [BODY-008 — Bodyless Request Replay Owner — FIXED](regression-guards/bodyless-request-replay-owner.md)
- [Request-Body Documentation Runtime Contract — FIXED](regression-guards/request-body-documentation-runtime-contract.md)
- [BODY-007 — Empty Request-Body Digest — FIXED](regression-guards/request-body-empty-digest.md)
- [BF-03 — The Generic Request-Body Gate Captures Without Materializing](regression-guards/request-body-gate-lazy-materialization.md)
- [BODY-002 — Request-Body Mutation Physical Cap — FIXED](regression-guards/request-body-mutation-physical-cap.md)
- [BF-02 — Over-Limit Request Bodies Are Rejected Without Draining](regression-guards/request-body-over-limit-immediate-reject.md)
- [BODY-001 — Request-Body Retry Framing Metadata — FIXED](regression-guards/request-body-retry-framing-metadata.md)
- [Request-Body Storage I/O Has a Separate Bounded Lane](regression-guards/request-body-storage-io-deadline.md)
- [A response-stage denial is decided from the RESPONSE, and a refused reply still terminates — FIXED](regression-guards/response-stage-denial-must-terminate.md)
- [Identity response encoding classification is shared across semantic consumers](regression-guards/identity-content-encoding-classification.md)
