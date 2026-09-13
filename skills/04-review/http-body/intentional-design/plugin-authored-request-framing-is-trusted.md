---
name: plugin-authored-request-framing-is-trusted
description: Use when reviewing whether generic request-header mutation must reject or overwrite Content-Length and Transfer-Encoding when no body processor owns a replacement body.
knowledge_type: intentional-design
---

# Plugin-Authored Request Framing Is a Trusted Configuration Choice

## Conclusion and scope

Edgion does not add a universal `Content-Length` / `Transfer-Encoding` deny-list
to generic request-header mutation. Gateway API `RequestHeaderModifier`,
`ProxyRewrite`, DSL, and Wasm are trusted, operator-authored extension surfaces;
an explicit framing mutation made through one of those surfaces remains the
configuration author's responsibility when no body processor owns a replacement
body.

This choice does not weaken the framing guarantees of the body subsystem:

- a committed buffered-body mutation retains its final length in
  `RequestBodyState` and every upstream attempt removes `Transfer-Encoding` and
  writes the matching `Content-Length`;
- an installed request-body stream processor selects Pingora's `Streamed` relay
  disposition, which derives protocol-appropriate H1/H2 framing; and
- Pingora continues to own external H1 transfer-coding admission, standard and
  dynamic `Connection`-nominated hop-by-hop sanitization, bodyless handling, and
  the final H1/H2 transport writer.

ExtProc remains a separate trust boundary: processor-returned pseudo, hop-by-hop,
and framing mutations are rejected by its existing protected-header policy. This
decision does not generalize that external-processor restriction to trusted local
configuration surfaces.

## Rationale and evidence

The generic `PluginSession::{set,append,remove}_request_header` methods intentionally
provide direct request-header mutation. Their current consumers include Gateway API
header modifiers, `ProxyRewrite`, DSL, Wasm, authentication header propagation, and
credential injection. Making those shared methods reject framing names would change
all of those surfaces and would turn a trusted extension API into a partial header
API without a demonstrated production incident.

The body paths already repair the metadata they actually invalidate.
`pg_upstream_request_filter::sync_body_framing_headers` reads
`RequestBodyState::modified_len_for_framing` on every upstream attempt, removes
`Transfer-Encoding`, and publishes the final `Content-Length`. For incremental
body processing, `EdgionHttpProxy::request_relay_plan` selects Pingora's
`RequestRelayPlan::streamed`; the fork resolves H1 chunked framing or H2 stream
termination after the application filter. The existing BODY-001 regression rule
records why mutation length outlives the plugin-readable body view.

The accepted boundary is therefore semantic rather than syntactic: the Gateway
guarantees framing for body behavior that it owns, but does not try to make every
possible explicit request-header program safe. The decision was confirmed by the
project owner on 2026-09-13 while reviewing
`unify-proxied-request-header-mutation-and-framing-policy`.

## Accepted cost and rejected alternatives

An operator can configure a generic header mutation that disagrees with an
otherwise unmodified request body. Depending on protocol and value, the observable
result can be a truncated upstream body, premature-body failure, upstream timeout,
or rejected H2 request. This is accepted as trusted-configuration misuse rather
than treated as an adversarial client capability.

The following alternatives are not adopted:

- plugin-specific framing deny-lists, which duplicate policy and inevitably drift;
- DSL- or Wasm-specific static restrictions, which cannot cover dynamic header
  names consistently and unnecessarily narrow trusted extension semantics; and
- unconditional final-boundary overwriting, which requires preserving additional
  attempt-stable ingress framing metadata and changes explicit HeaderModifier
  behavior even when the body subsystem did not alter the message.

This decision does not authorize a body transformer to publish bytes without
matching framing, nor does it allow a new untrusted extension surface to inherit
the same trust assumption automatically.

## Verification and re-evaluation

Current source evidence:

- `edgion-gateway/src/plugins/runtime/session_adapter.rs` implements generic
  request-header mutation directly on the Pingora session;
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`
  rebuilds framing for committed buffered-body mutations on every attempt;
- `edgion-gateway/src/routes/http/proxy_http/mod.rs` selects streamed relay when
  a request-body stream processor is active; and
- `edgion-gateway/src/plugins/http/ext_proc/mutation.rs` preserves the distinct
  external-processor protected-header boundary.

Reconsider this tradeoff if any of the following occurs:

- route or plugin configuration becomes writable by an untrusted tenant;
- a real incident attributes body corruption or request desynchronization to a
  generic framing-header mutation;
- a new body mode cannot express correct retry-stable framing through the existing
  buffered or streamed contracts; or
- Pingora introduces a canonical final request-framing API that can preserve
  explicit extension semantics without parallel policy in Edgion.

## Resolution references

- `unify-proxied-request-header-mutation-and-framing-policy` — 2026-09-13
  (`tasks/todo/unify-proxied-request-header-mutation-and-framing-policy.md`,
  accepted tradeoff; no runtime change).
