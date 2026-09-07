---
name: header-hmac-condition-verifier-prebuilt
description: Use when reviewing header-only hmacAuth conditions for per-request verifier construction or resolved credential-map cloning. Gateway consumers compile schema conditions once and retain one HmacVerifier per runtime snapshot.
knowledge_type: regression-guards
---

# Header-only HMAC condition verifier is prebuilt — FIXED (cl-05)

## Conclusion

Confirmed and fixed on 2026-08-24. Header-only `hmacAuth` conditions previously
called `HmacVerifier::new` from the shared condition evaluator on every request.
The constructor cloned `resolved_credentials`, `algorithms`, and `enforce_headers`,
so work and sensitive byte copies grew with both credential-table size and QPS.

The Gateway now owns `CompiledCondition` and `CompiledPluginConditions`. Runtime
construction builds a header-only `HmacVerifier` once; request evaluation borrows
that verifier. The compiled container preserves skip-OR, run-AND, unresolved-reference,
`negate`, descriptor, and `cond_skip` behavior.

## Required consumer coverage

The following product paths must retain compiled conditions:

- all four conditional entry wrappers;
- request and upstream-response secondary body gates;
- compiled dye rules;
- RequestRestriction synchronous predicates;
- RequestAccessPolicy synchronous secondary conditions.

Body-validating HMAC remains a separate asynchronous predicate in
RequestRestriction and RequestAccessPolicy because its body-read failure policy is
not synchronous condition semantics. D-27 continues to reject body-validating HMAC
at entry level.

`ConditionExt::evaluate` retains a defensive schema-level header fallback for tests
and future internal callers. It calls `HmacVerifier::verify_headers_config`, which
borrows the schema configuration and credential map; it must never construct a
verifier or clone the map.

## Review guard

Before re-reporting, search all `HmacVerifier::new` call sites. They are valid only
in runtime/plugin construction: compiled header conditions, the standalone HmacAuth
plugin, and the two body-HMAC predicate builders. A constructor call from
`evaluate`, `run_request`, or another per-request function is a regression.

The regression test
`compiled_header_hmac_reuses_one_credentials_arc_across_evaluations` locks the
compiled verifier's credential `Arc` identity across repeated evaluations.

## Re-evaluation triggers

Re-open if a new condition consumer evaluates schema `PluginConditions` directly,
or if a new HMAC mode introduces request-dependent verifier configuration that
cannot be resolved while building the immutable runtime snapshot.

## Resolution references

- `cl-05` — 2026-08-24.
