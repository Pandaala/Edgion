---
id: ARCH-050
status: candidate
kind: architecture-debt
priority: P3
partition: PART-208
analyst: /root/analyze_conversion
independent_verification: confirmed by /root/analyze_outbound
---

# Reuse request-entry configuration validation within one resource-validation invocation

Resources validate_plugins_with_waf owns immutable schema-layer orchestration
for validate_plugins and validate_plugins_before_waf_resolution. Every enabled
request entry reaches independent validation_error calls at validate.rs:540
(bounded-body prepass, before category/body checks), :580 (configuration/alias/
stage pass) and :745 (body-setting pass, before the body presence check at :752).

DSL dispatch at edgion_plugin.rs:775 calls DslConfig::validation_error, which
runs compile_and_validate at dsl.rs:369-410. The leaf resolve_script performs
real source compilation and bytecode deserialization or bytecode decoding/static
validation. Source/response-body/link checks and Redis name scans repeat too.
An enabled valid request DSL without a body setting therefore runs this entire
configuration pipeline three times per resource-validation invocation. Additional
DSL semantic checks can compile separately and remain outside this finding.
The actual Controller preparse invokes before-WAF validation at its
handlers/edgion_plugins.rs:225. This is configuration work, not request execution.

Gateway plugin_runtime.rs:1066-1078 already collects request configuration
results once in a call-local vector and reuses them. Resources passes ask the
same stateless question without retaining its answer for their shared immutable
invocation. This multiplies source compilation and temporary allocations on
resource reprocessing. No measured latency or incorrect acceptance is claimed.

Compute a configuration result once per enabled known request entry, indexed
by original array ordinal, and reuse it in the three existing passes. Emit
diagnostics at their existing positions rather than returning an early error
list; retain ordering, wording, multiplicity and indices. Storage and error
handling stay bounded by that entry array and die with the invocation. Keep
PluginValidation stateless and IO-free: no resource field, serialized compiled
state, global/shared memo or cross-generation cache. Independent invocations
validate their current source afresh. Response material-free descriptors and
additional DSL stage/body semantic checks remain unchanged. No Gateway factory
or generic plugin registry is needed.

Checking category before validation in the bounded prepass and body presence
before the body pass is a smaller alternative, but still repeats configuration
validation for entries participating in multiple passes. Invocation-local reuse
addresses the common result owner directly. ARCH005 addresses typed Gateway
transport-admission publication, a different cause. WAF and condition compilation
have different owners and are excluded.

Preserve real leaf compiler validation in Resources and independent Gateway
constructor/VM boundaries. Unknown types are rejected before other validation,
including disabled unknown entries; known disabled entries remain inert.
Configuration errors suppress body-policy errors; invalid bounded-body entries
are skipped before alias ownership and later dye/stage checks. Preserve aliases,
condition/dye order, cross-entry restrictions, body and stage capability, request
storage, response streaming prohibitions, and current failure/redaction policies.
Stateless implementations remain the graded rule; orchestration-local reuse
does not relax DSL/WAF Controller/Gateway validation parity.

Existing compile_plugins parity, disabled-condition, intrinsic body, DSL body/
stage and response descriptor precedence tests were inspected. The principal
parity case sorts errors and cannot alone prove diagnostic order. Future
verification should preserve ordered mixed-invalid errors, config-before-body
and bounded-before-alias precedence, source ordinals, disabled known/unknown
semantics and independently sourced source/bytecode paths. Show once-per-active
known-request-entry configuration validation per invocation while the real leaf
compiler still runs; source changes validate again and equal aliases/text do not
merge entries. Retain independent semantic checks and existing private unit
access rather than mock-only validators or widened visibility.

Expected benefit is removing two redundant configuration executions on the
demonstrated path and clarifying their shared result owner. No request-path
speedup is claimed. Full assigned source and exact supporting test/original/call
read scopes and hashes belong to REPORT-PART-207.json and REPORT-PART-208.json.
Static review only; no builds, tests, benchmarks, runtime experiment or production
changes. Independent verification is pending.
