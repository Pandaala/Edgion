# Evaluate system DNS query input ownership

Status: deferred for user decision. No runtime change.
Source baseline: 53ac031ac; locked Tokio 1.53.1.

## Evidence and candidate

Common `dns/system.rs::resolve_system` first returns bare IP literals directly.
Otherwise it formats `domain:0` and passes the borrowed String to Tokio
`lookup_host`. Tokio's string adapter tries SocketAddr parsing, then copies the
formatted input into a blocking task. For ordinary hostnames this leaves both
the caller's formatted owner and the task's owned input during resolution.

Tokio's `(host, port)` adapter instead parses bare IPv4/IPv6, then owns the host
for its blocking task. Using `(domain, 0)` could avoid the caller's formatted
String for ordinary hostnames; it does not eliminate the blocking task's owned
input or the system resolver's work and allocations.

A blind substitution is not equivalent. `[::1]` currently becomes `[::1]:0`
and reaches SocketAddr parsing without a blocking lookup. The tuple adapter
receives a bracketed host and falls through to OS resolution; this is not a
claim that every OS rejects it. Rust 1.96.1's socket parser also accepts numeric
IPv6 scope IDs, which a bracket-strip followed by IpAddr parsing does not cover.
The returned IP projection currently discards socket port/scope information.
Generic System resolution, StandardDns with empty servers and HTTPDNS System
fallback all reach this helper without universal bracket normalization.

A small conservative alternative exists: retain the current path for empty,
colon-bearing or bracket-bearing inputs and evaluate tuple lookup for remaining
hostnames, keeping the bare-IP shortcut first. This adds an input scan/dispatch;
it is a candidate, not an approved implementation. Keeping the current code is
also valid. No operational bottleneck or throughput improvement is established.

## Required evidence before implementation

- Trace the locked Tokio and supported-platform standard-library adapters to
  prove the same host/port reaches the OS for each optimized input class.
  Preserve errors and their existing DnsError mapping; do not silently narrow
  accepted input or add a resolver/cache/normalization policy.
- Cover bare and bracketed IPv4/IPv6, numeric scoped IPv6, empty input, malformed
  brackets, colon-bearing text, whitespace, non-ASCII names and ordinary names.
  Preserve literal fast paths, address order, first-occurrence deduplication,
  all-address output, fallback selection and no-address handling.
- Keep resolver work off async workers, cancellation ownership and existing
  egress classification/observation boundaries. Add no query-time tracing or
  raw hostname/error diagnostics.
- Compare input allocations/copies and added scanning separately from DNS
  latency, including short/long names and concurrent misses. Do not infer a
  throughput or RSS gain from source-level allocation counting alone.
- Use IMPL with an exact allow-list after selection. Existing System tests do
  not cover the full input matrix or establish a new dispatch's equivalence.

This differs from [standard resolver preparation](dns-03-standard-resolver-preparation.md)
and [address-result ownership](dns-01-address-result-ownership.md). The existing
[bare IPv6 guard](../../skills/04-review/backends/regression-guards/resolve-domain-ipv6-literal.md)
remains applicable; this task does not reopen the fixed literal defect.

Fresh unchanged-code verification passed 4 Common System DNS and 11 Common
egress tests, formatting, workspace all-target check/Clippy and agent-doc
validation, with existing warnings. No new tests, candidate implementation,
allocation experiment, controlled DNS fixture or full suite ran.
