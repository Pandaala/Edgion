# CSRF static integration coverage

Run the shared local topology with:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh -r EdgionPlugins -i Csrf
```

The existing token and cookie cases retain their shared Gateway. The additional
`method_rewrite_protection_boundary` case owns a dedicated GatewayClass/config,
Gateway and listener registered as `EdgionPlugins/Csrf/MethodBoundary` in
[ports.json](../../ports.json). It executes 18 real
ordered-chain probes with the same signing-key Secret and existing test-http
backend. Global and Gateway checks are host-conditioned; Route and BackendRef
filters use real resource attachments.

The origin fixture exposes only call/read/write counters and the resulting
method/path/effect. Its fixed synthetic session cookie permits authenticated
POST writes. GraphQL query transport reads; mutation transport writes. Token
values are neither retained nor printed. Rejected probes must produce zero
origin call/read/write deltas; allowed probes prove their actual origin effect.

This composition case uses local fixture resources and is not registered in
Kubernetes mode. Existing Kubernetes CSRF token cases remain available; this
run does not claim Kubernetes or browser-based composition coverage.
