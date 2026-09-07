---
name: dsl-ssrf-guard-naming
description: Historical review evidence for RFC / DSL SSRF guard naming; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Dsl ssrf guard naming

## Recorded conclusion

Renamed DSL `is_private_ip_host` → `is_ssrf_blocked_ip_literal` (+ tightened doc-comment naming RFC 1918 / 4193 / 6874 anchors): the old name read as a generic classifier and hid the load-bearing SSRF-block intent at the single `http.call` caller (`builtins.rs:866`, gated by `runtime.block_private_ips`). `ip_literal` keeps the "hostnames are not resolved" limitation in the name. Private free fn, never exported — no wire/behavior change. Skills docs `dsl-engine-security-model.md` §7 + `03-coding/dsl/http-call.md` updated

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `naming-http1-09` — 2026-05-21.
