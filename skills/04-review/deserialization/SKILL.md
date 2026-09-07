---
name: review-deserialization
description: Review knowledge for deserialization, grouped by decision type.
---

# deserialization review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Plugin config `Regex::new` without DFA size_limit — ACCEPTED-TRADEOFF (not fixed)](intentional-design/plugin-config-regex-dfa-size-limit.md)

## Regression guards

- [Typed ConfigData policy fields use advisory unknown-field handling](regression-guards/configdata-typed-policy-unknown-fields-are-advisory.md)
- [DSL Bytecode Deserialize Safety — OPEN / IN-PROGRESS](regression-guards/dsl-bytecode-deserialize-safety.md)
- [LinkSys auth resolved Secrets must be cleared before resolution](regression-guards/linksys-auth-resolved-secret-clear-before-resolve.md)
- [Plugin auth resolved credentials must be rebuilt by the Controller](regression-guards/plugin-auth-resolved-credentials-clear-before-resolve.md)
- [Runtime-ignored fields may still have advisory diagnostics](regression-guards/runtime-ignore-does-not-mean-no-unknown-field-diagnostic.md)
- [Status seeding must preserve ingest-only sidecars](regression-guards/status-seed-must-preserve-ingest-sidecars.md)
- [Unknown-Field Tests Need Clean and Typo Coverage](regression-guards/unknown-field-tests-need-clean-and-typo-coverage.md)
