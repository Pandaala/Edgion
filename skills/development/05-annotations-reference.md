---
name: annotations-reference
description: Use when changing, reviewing, or debugging Edgion annotation keys, listener option keys, or reserved system-injected metadata. Helps choose the right `edgion.io/*` reference by placement and warns about stale legacy keys.
---
# Annotations And Option Keys

Use this skill when the task involves any `edgion.io/*` key and you need to answer:

- where the key is configured
- whether it is a real `metadata.annotations` field or an `options` field
- whether it is safe for users to set directly
- which keys are current vs legacy/stale

## Choose The Right Reference

- For `metadata.annotations` on `Gateway`, `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `EdgionTls`, `Service`, or `EndpointSlice`, read [references/annotations-metadata.md](references/annotations-metadata.md).
- For `listener.tls.options`, `BackendTLSPolicy.spec.options`, or related `edgion.io/*` labels, read [references/annotations-options-and-labels.md](references/annotations-options-and-labels.md).
- For controller-injected, gateway-injected, ACME/internal, or test-only keys, read [references/annotations-system-and-test-keys.md](references/annotations-system-and-test-keys.md).

## High-Risk Gotchas

- The current prefix is `edgion.io`, not `edgion.com`. If you see `edgion.com/enable-http2`, treat it as stale.
- The current stream-plugin key used by Gateway/TCPRoute/TLSRoute code paths is `edgion.io/edgion-stream-plugins`. Older docs and a few historical examples used `edgion.io/stream-plugins`.
- `edgion.io/hostname-resolution` and `edgion.io/sync-version` are reserved system-managed annotations. Do not hand-author them in manifests.
- `edgion.io/cert-provider` and `edgion.io/client-certificate-ref` are `options` keys, not `metadata.annotations`.

## Quick Review Checklist

- Confirm the key lives at the correct path: `metadata.annotations`, `listener.tls.options`, or `spec.options`.
- Confirm the resource kind is one of the currently implemented consumers.
- Check whether the value is a boolean string, integer string, YAML string, or `namespace/name` reference.
- Search both code and examples before renaming a key:

```bash
rg -n 'edgion\.io/' src examples docs skills
```

- If the task updates docs or skill entry files, run:

```bash
make check-agent-docs
```

## Related

- [04-config-reference.md](04-config-reference.md)
- [../testing/03-debugging.md](../testing/03-debugging.md)
- [../../docs/zh-CN/dev-guide/annotations-guide.md](../../docs/zh-CN/dev-guide/annotations-guide.md)
