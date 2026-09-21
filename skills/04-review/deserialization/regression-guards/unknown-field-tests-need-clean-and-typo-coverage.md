---
knowledge_type: regression-guards
---

# Unknown-Field Tests Need Clean and Typo Coverage

Unknown-field detection is a schema-dispatch contract, not only an error-path
contract. A focused regression test for a tagged resource family must prove
both directions for every supported variant:

1. A representative clean document produces no findings.
2. The same document with one controlled typo reports the exact expected path.

Checking only the typo case can hide a schema branch that reports the expected
typo together with false positives on valid fields. Checking only one variant
can hide drift in a newly added or renamed discriminator branch.

Public validation-endpoint coverage must additionally assert the
`unknownFieldCheck` discriminator. `unknownFields` alone cannot distinguish a
performed walk with no findings from a resource kind for which the walk was
not available.

For ConfigData and LinkSys, keep the production-schema unit matrices aligned
with every typed variant/provider, and retain an integration assertion for the
Admin validation response shape.

The clean half of the matrix only holds if every *known* field in the fixture is
schema-valid for its current typed representation. A fixture whose known field no
longer deserializes fails before the advisory assertion, so the typo half proves
nothing either: both directions collapse into the same type error and the walker
for that variant is left unverified. Assert on the walker's findings, never on
"the test compiles and runs".

The LinkSys/redis fixtures are the reference case. `RedisTimeout.connect` is typed
`Option<String>` (`edgion-resources/src/resources/link_sys/redis.rs`), so a numeric
`timeout.connect: 10` failed serde with `invalid type: integer 10, expected a string`
in both `TYPO_LINK_SYS` (`edgion-controller/src/conf_mgr/conf_center/common.rs`) and
`link_sys_body` (`edgion-controller/src/api/namespaced_handlers.rs`), turning the Admin
create assertion into HTTP 400. The decisive cause is the Rust field type, not the
GEP-2257 `#[schemars(pattern(...))]` bound on it — the CRD marks the provider `timeout`
subtree `x-kubernetes-preserve-unknown-fields`, so that pattern is not an admission
gate for redis. The repair is to write the canonical duration form while keeping the
deliberate `conect` misspelling and its warning assertions untouched — the fixture is
corrected, the advisory semantics and schema strictness are not relaxed. Prefer a
literal well inside the provider bounds (`3s`, matching every other redis fixture in
the repository) over one sitting exactly on `REDIS_MAX_CONNECT_TIMEOUT`.

Re-evaluation trigger: whenever a duration, numeric, or enum field referenced by an
unknown-field fixture changes its typed representation, re-run the clean and typo
tests for that variant and update the fixture literal rather than loosening the schema.

## Resolution references

- `UFD-06` — 2026-08-22.
- `linksys-timeout-advisory-test-fixtures` — 2026-09-21.
