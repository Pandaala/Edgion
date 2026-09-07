---
name: h2-writer-capacity-stall-after-response
description: Historical review evidence for Pingora fork / H2 request-body pump; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 writer capacity stall after response

## Recorded conclusion

An upstream that flags END_STREAM and then withholds request-body flow-control window without resetting must not block the write forever: the pump awaits that write inline, so the duplex loop stops delivering the response and the client is answered never. Wire END_STREAM plus a stalled write authorizes abandoning the upload and DELIVERING the response; failing the exchange, bailing on END_STREAM alone with no timer, adding a `Notify` to `StreamRecord`, cancelling `write_body` on a probe tick, and making the probe interval a peer option are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-writer-capacity-stall-after-response.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-writer-capacity-stall-after-response.md)

## Resolution references

- `H2-007` — 2026-08-27 (fixed in pingora fork `edgion_v3`).
