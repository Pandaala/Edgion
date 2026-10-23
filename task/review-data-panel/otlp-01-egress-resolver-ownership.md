# Evaluate OTLP egress resolver ownership

Status: deferred for user decision. Priority: P3. Source baseline: 1dfd9d175.
No runtime or dependency change.

## Evidence and value boundary

Terminal access logging reaches the LinkSys OTLP pipeline. Its prepared transport
owns a ResolverSpec, but transport.rs::dial constructs a canonical EgressResolver
with zero TTL on every connector invocation and active health probe. Common's
constructor allocates an Arc<Inner> containing the spec, policy, observer and an
empty HashMap/RwLock. The empty HashMap does not establish a bucket allocation.
Zero TTL already bypasses memo lookup and insertion entirely.

connect clones the spec into the connector and again into each dial future;
probe also clones it. OtlpApply::build and the ordinary client constructor use
ResolverSpec::System, whose clone needs no heap allocation. StandardDns owns a
Vec<String>; HttpDns clones an Arc to its prepared generation. Those heavier
variants are available through explicit test-util resolver injection, not the
ordinary production OTLP construction path. Do not attribute their costs to all
production exports.

Reusing a generation-owned egress resolver could avoid repeated wrapper
construction. It also retains that owner between dials and introduces shared
handle operations. Connections are reused, and active probes are configured
with a 10-second base interval and 30-second maximum backoff. This is dial/probe
work, not per-record or per-request work. No benchmark or demonstrated operational
benefit currently justifies choosing a representation.

## Decision and semantic boundaries

- Keep the existing local wrapper unless representative measurements justify
  changing this low-frequency path.
- Reuse an immutable resolver owned by PreparedTransport, after explicitly
  resolving policy-sampling and lifetime requirements.
- Consider a lightweight zero-TTL operation in the canonical egress owner only
  if multiple consumers justify it. Do not duplicate DNS resolution, address
  filtering or observation in the OTLP adapter to remove one allocation.

Currently default_block_private is read on each dial. Hoisting the entire resolver
would capture it at construction instead. The ordinary CLI installs defaults
before clients; this is not evidence of a supported production policy hot-reload
feature. Nevertheless, the current sampling boundary must be preserved or
explicitly decided. The production_dns_case integration fixture changes policy
between newly constructed clients; it does not prove same-owner policy changes.

Retain zero TTL, fresh resolution on every dial, full admitted-address order and
fallback, mapped-IP classification, existing test exceptions, and the Gateway's
canonical rejection observer. No hostname fallback, proxy environment or redirect
path may be introduced. Retain original authority/SNI, TLS/auth provenance,
fixed connect_failed errors, deadlines, TCP-level health semantics, empty-Export
publication admission, generation replacement and retirement cancellation.

This is separate from [explicit DNS runtime preparation](dns-03-standard-resolver-preparation.md),
which concerns Hickory pool/cache/hosts lifetimes; [cached egress address ownership](egress-01-cached-address-ownership.md),
which concerns memo values; and [SDK logger reuse](log-03-otlp-logger-generation-cache.md),
which concerns emission and shutdown. None authorizes another cache here.

## Acceptance after user selection

Compare wrapper allocations, spec copies, atomic handle operations and retained
memory across idle/unused generations, real reconnect rates and probe cadence.
Separate System production results from explicit StandardDns/HttpDns scenarios.
Include concurrent probes/reconnects, failed/cancelled dials, replacement and
retirement. Fewer constructors alone do not prove improved throughput or RSS.

Prove fresh DNS rebinding checks and observer behavior for mixed, blocked and
failed answers, including mapped IPs and ordered fallback. Test old/new generation
isolation and the selected policy-sampling contract. Use controlled transport
integration for live dial/TLS claims; lazy construction and unit tests cannot
establish them. Preserve the shared lifecycle and existing redaction/diagnostic
boundaries, without new metrics or raw endpoint logging.

Fresh unchanged-code verification passed 19 OTLP provider unit tests. These cover
existing local ownership/configuration behavior, not shared resolver equivalence.
No prototype, benchmark, full suite or live DNS/TLS/collector test ran.

Original authority: [OTLP provider ownership and safe transport](../../skills/04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md).
