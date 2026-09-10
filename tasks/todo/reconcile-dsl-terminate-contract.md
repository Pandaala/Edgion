# Reconcile the DSL Terminate Contract

## Meta

- Status: needs-discussion
- Priority: P2
- Category: architecture and documentation

## Conflicting evidence and requirements

- `edgion-gateway/src/plugins/http/dsl/tests/terminate_tests.rs` describes abrupt termination without a response; its four tests actually assert the VM signal ErrTerminateRequest.
- English and Chinese DSL API terminate sections now describe HTTP 502 / gRPC fallback. Both language-syntax.md section 6.3 pages and the target test comment still promise silent closure without a response. The API correction describes observed behavior; it does not establish a DSL exception to Design F.
- Parser/compiler emit ReturnTerminate and `edgion-gateway/src/plugins/http/dsl/lang/vm.rs:1425` returns ErrTerminateRequest. execute_safe records the decision but does not write a response.
- `edgion-gateway/src/runtime/server/error_response.rs:396` converts missing-response termination into HTTP 502 / gRPC INTERNAL and PluginTerminateWithoutResponse. The response-stage handler in `session_adapter.rs:576` likewise records the bug and applies a 502 denial.
- Frozen `edgion-tests/src/client/suites/edgion_plugins/dsl/dsl.rs:1364` explicitly tests 502 when terminate triggers and 200 otherwise.
- `skills/00-design-rules.md` section F treats bare ErrTerminateRequest as a plugin bug and forbids normal plugins from relying on that fallback. Current code and tests do not authorize a DSL exception.
- `edgion-resources/src/filters.rs:40` retains weaker SHOULD/possible-exception wording for bare termination. Reconcile that comment as evidence; it does not override Design F or grant a DSL exception.

## Required resolution

First resolve the public DSL terminate contract against canonical termination rules.
Do not simply rewrite documentation to bless the fallback, or infer intended silent-close
behavior from historical prose. Record the governing requirement and its justification.
Then align the target test description and bilingual API/syntax documentation.

Any needed runtime/language change requires a separately frozen cross-layer allow-list,
a plan-first implementation and independent review. Preserve existing tests as observed
behavior evidence until an authorized change is defined. Do not modify existing integration
tests; record any needed new end-to-end coverage as a separate deferred task.

## Verification

- Retain the four VM termination cases and exact decision-log coverage.
- Use existing 502/200 integration scenarios as observational evidence, not authority for an exception.
- Preserve canonical access-log ownership, sensitive-data handling and failure-policy boundaries.
- Run relevant scoped tests and applicable project guards for the eventual resolution.

The separate response-stage grpc_error documentation correction was completed by external
work. It does not resolve this terminate contract conflict; do not recreate the closed task.
