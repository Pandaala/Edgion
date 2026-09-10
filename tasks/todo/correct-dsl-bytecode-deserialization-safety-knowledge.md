# Correct DSL Bytecode Deserialization Safety Knowledge

## Meta

- Status: pending
- Priority: P2
- Category: documentation accuracy

## Evidence

The review page `skills/04-review/deserialization/regression-guards/dsl-bytecode-deserialize-safety.md`
still contains incorrect resource-safety claims, verified against commit `f8c265eb`:

- `edgion-dsl/src/bytecode.rs:344` defines an ordinary derived-deserialization `Regex(String)` constant. Its decode path at line 390 checks length, base64, JSON and version; serde does not compile the regex.
- Compiler `edgion-dsl/src/compiler/emit.rs:90`, Validator `edgion-dsl/src/validator.rs:127` and Gateway `edgion-gateway/src/plugins/http/dsl/lang/vm.rs:364` explicitly set regex size and DFA limits to 1 MiB. Claims that these limits are absent are obsolete; the existing `skills/04-review/dsl/regression-guards/dfa-size-limit.md` is relevant evidence.
- The locked serde_json 1.0.151 dependency defaults to a recursion depth of 128 and enforces it in its deserializer. The DSL decode path does not disable this limit. The page's unlimited-default-recursion claim is incorrect; this correction is not a proof of safety for arbitrary input.
- The page should distinguish existing property tests from actual fuzz coverage and must not rely on the removed issue-19 task as current evidence.

The external correction in `939565dd` addressed transport/trust-boundary prose only.
Do not restore that completed task or undo its correction; this issue concerns the remaining
independent inaccuracies.

## Scope and acceptance

Revalidate the statements against current code and the locked dependency, then correct the
owning knowledge page and its deserialization index summary. Preserve useful historical
evidence while clearly separating it from current guarantees and remaining test gaps.
No runtime, schema, compiler or integration-test change is requested.

- Read applicable repository rules and original knowledge sources before editing.
- Independently review the corrected guarantees and cited implementation.
- Run the documentation validator and relevant project guards.
- Keep existing integration tests unchanged; record any genuinely needed new integration coverage as a separate task for later.
