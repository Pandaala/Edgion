# B0281 — HTTP DSL VM Defense

- Pipeline state: analysis-ready; no mutation authorized.
- Active anchor: `edgion-gateway`; baseline `9e3a276b`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/vm_defensive_tests.rs`.
- Analyst: `b0281_vm_analysis`; examined-no-change, simple. Seven manual-bytecode tests accurately exercise VM stack/constant/regex index defenses and JSON-select construction validation. Compiler, Validator, resolve_script, execute_safe and sibling defense/property/error coverage inspected; no style-only churn justified.
- Target blob: `3eb646c13c034b24beb4544e444316710a5eb163`, unchanged at report completion.
- Separate candidate finding: deserialize knowledge still describes Regex(String) as serde-time compilation and omits existing compilation limits; independently validate after serial promotion without resurrecting the externally completed trust-boundary task.
- B0280 retains serial verification lane; existing integration and user handoff/WAF paths protected.
