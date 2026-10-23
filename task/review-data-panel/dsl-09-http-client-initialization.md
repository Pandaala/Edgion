# DSL HTTP client initialization

Status: deferred for user decision. No runtime change.
Source baseline: 168059588.

## Evidence and scope

`edgion-gateway/src/plugins/http/dsl/runtime.rs::dsl_http_client` owns four
process-global OnceLock clients, selected by private-address blocking and
redirect permission. Each variant is constructed only on its first use.
`lang/builtins.rs::call_builtin_http` selects that client after argument and
DSL header validation, before the opaque upload size check. String/Nil bodies
allow redirects; opaque Body uploads select the no-redirect variant.

Construction uses the canonical Common outbound factory, including the actual
fallible reqwest builder.build call. The DSL wrapper expects construction to
succeed. An unwinding construction panic during VM execution falls inside
execute_safe's catch_unwind and configured error policy. Moving construction
to plugin preparation changes that failure boundary; this is distinct from
transport/response failures returning the established status-zero map.

The first request for a variant performs synchronous construction; concurrent
first users can wait for the same OnceLock initialization. Warm requests reuse
the global client and connection pool. This is first-use work, not a claim
that every call rebuilds a client or that construction performs network IO.
No cold-start latency or request-throughput benefit has been measured.

## Decision requested

Investigate selective configuration-time preparation, including whether the
plugin should retain prepared client references, and choose an explicit cold
failure policy. Retaining lazy initialization would need an explicit resolution
against design rule U's preparse/config-resolve requirement; current behavior
is not evidence of an accepted exception.

CompiledScript::references_builtin(BuiltinId::HttpCall) already provides a
conservative presence scan for source and precompiled scripts. It does not
prove reachability or which body variant a call will use. Preparing both
variants for the configured private-address policy initializes resources even
when a branch is never executed. Blindly preparing all four variants also
initializes the unused private-address policy. No selection strategy is chosen.

## Acceptance before implementation

Use IMPL with a frozen source allow-list. Compare cold construction, concurrent
first-use and warm-call costs in isolated processes so global OnceLocks cannot
hide initialization. Separate allocation/retention and constructor latency from
network latency; do not infer throughput from a microbenchmark.

Preserve process-global pool identity across plugin instances and reloads;
do not replace these with per-plugin clients. Keep the canonical factory,
proxy-environment disabling, private-literal and resolved-address checks,
redirect limits, opaque-body non-replay policy, deadlines and body budgets.
Do not add request logs/metrics or widen this into an outbound client redesign.

The canonical factory captures the process-wide HTTP/2 response-header-list
cap. Preparation must occur after Gateway bootstrap installs that restart-only
value. Verify runtime/executor requirements and configuration-load contexts
before moving construction. Define how construction failure affects plugin
publication, last-known-good behavior and retry, without silently converting
it into a status-zero HTTP result or losing panic isolation.

Cover scripts without HttpCall, conditional/unreached calls, all four variants,
source/precompiled parity, multiple instances and reloads, and request/response
stage entry. Preserve argument-validation and opaque-upload error precedence.
Use isolated constructor tests and synthetic response tests without live
network IO; existing ignored integration tests remain unchanged. Existing DSL
coverage is a baseline, not proof of a new initialization lifecycle.

Original authority: `skills/00-design-rules.md` section U,
`skills/01-architecture/00-common/05-unification-points.md` section 1,
`skills/01-architecture/02-gateway/dsl-engine.md`, and
`skills/04-review/dsl/intentional-design/http-call-body-truncate-to-max-string-len.md`.
