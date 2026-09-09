# Reconcile HTTP Context Layout Expectations

## Meta

- Status: pending
- Type: test
- Priority: P3
- Source: Gateway campaign B0207 / D0119

## Evidence

Independent `ctx::` execution at `aba8fd24` reports 91 passed, three failed and
one ignored. The unchanged `ctx.rs` blob is
`4f71d68cf6cb4b54faf425b8678bb7964246bea3`.

- PluginAuxState is 248 rather than 232 bytes: `90a45683` added the two-Arc,
  16-byte MirrorBodyBudget inside MirrorState; the assertion still predates it.
- UpstreamJumpTarget is 72 rather than 80 bytes: `9b6b6fa0` replaced the
  DirectEndpointPreset bool/String shape with BackendTlsPolicyLookup.
- EdgionHttpContext is 1120 rather than 1112 bytes: `8cbbed6c` changed the
  embedded SelectedGrpcBackend policy lookup and its verified size from 24 to
  32 bytes after the context expectation's `3b76481f` update.

B0207 changes only TLS proxy function implementations/tests, not any embedded
layout type or Cargo configuration. Independent grpc backend and ordinary context
budget tests each pass; the latter remains within its 1280-byte budget.

## Acceptance

- Exact proposed scope: `edgion-gateway/src/ctx.rs`, its three stale assertions
  and corresponding layout comments. Revalidate HEAD and design costs first.
- Preserve pointer-size qualification, overall budgets and allocation checks;
  do not merely raise limits or remove failing assertions.
- Rerun the complete context tests and relevant backend layout tests. The
  allocation-free test currently panics at the size assertion before reaching
  the allocation assertion; zero allocation has not been verified by that run.
- Keep integration tests unchanged. This is separate from TLS ownership work.
