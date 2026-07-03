# FIPS Mode (BoringSSL)

> **🔒 Compliance feature**
>
> FIPS mode runs the Gateway data plane on a FIPS-validated BoringSSL crypto module and makes
> downstream (client → Gateway) TLS default to the FIPS-approved cipher and curve set. It is a
> **build/image concern**, not a runtime toggle: the validated module is compiled in when the
> Gateway image is built with the `fips` cargo feature. There is no per-listener or per-connection
> FIPS switch.

FIPS mode targets deployments that must run in a FIPS-compliant posture (US federal, regulated
industries). When the Gateway binary is a FIPS build, two things hold:

1. **Validated module** — the linked crypto is the FIPS-validated BoringSSL module
   (`boring::fips::enabled()` is true).
2. **Approved-by-default TLS** — downstream server TLS defaults to the FIPS-approved TLS 1.2
   cipher set and ECDH curves, unless an operator overrides ciphers explicitly on a listener.

This applies to the **BoringSSL** backend only (the default TLS backend). FIPS is not available on
the `rustls` backend.

## What a FIPS build enforces

On a FIPS build, downstream TLS-terminating listeners apply the approved default unless overridden:

| Aspect | FIPS-approved default |
|--------|-----------------------|
| TLS 1.2 cipher list | `ECDHE-ECDSA-AES128-GCM-SHA256`, `ECDHE-RSA-AES128-GCM-SHA256`, `ECDHE-ECDSA-AES256-GCM-SHA384`, `ECDHE-RSA-AES256-GCM-SHA384` (excludes ChaCha20, CBC-SHA1, RC4, 3DES) |
| ECDH curves | `P-256`, `P-384`, `P-521` |
| Minimum TLS version | Unchanged — BoringSSL already defaults to TLS 1.2+, which is FIPS-acceptable |
| TLS 1.3 suites | Unchanged (see [Known limitations](#known-limitations)) |

An operator override always wins: if a listener's `EdgionTls.spec.ciphers` is set, that list is used
instead of the FIPS default (the approved curves still apply). See
[Per-listener override](#per-listener-cipher-override).

## Enabling FIPS

FIPS is enabled at **image build time**, not at runtime. Release/CI Docker builds compile the
Gateway with the `fips` cargo feature and bake in the prebuilt validated module. The default
workspace build and local `cargo build` do **not** include FIPS — this keeps local/dev builds lean
and always buildable.

There is no config field or environment variable that turns FIPS "on" — you deploy a FIPS image.
The config flags below control **assertions and policy**, not whether the validated module is
present.

## Configuration

Two gateway-local (process, non-cluster-synced) startup flags govern FIPS behavior. Both live in the
Gateway YAML config and have environment-variable equivalents. See the full
[Gateway Configuration](../../configuration/gateway-config.md) reference.

### `require_fips` — fail-closed startup guard

Assert at startup that the running binary is a FIPS build. Use this on nodes that **must** be FIPS
to catch a misconfigured (non-FIPS) image before it serves traffic.

| Field | Type | Default | Env | Description |
|-------|------|---------|-----|-------------|
| `require_fips` | `bool` | `false` | `EDGION_REQUIRE_FIPS` | Require the FIPS-validated crypto module to be present at startup |

- `require_fips: false` (default) — starts normally regardless of whether the binary is a FIPS build.
- `require_fips: true` on a **FIPS build** — guard passes; approved defaults apply automatically.
- `require_fips: true` on a **non-FIPS build** — the Gateway logs a clear error and **exits
  non-zero** at startup (fail-close). No silent downgrade.

It is a gateway-local field, not a cluster-synced one, on purpose: a bad value cannot be hot-pushed
to crashloop an entire fleet, and it surfaces on the very first run.

```yaml
# edgion-gateway.yaml
require_fips: true
```

```bash
# or via environment (truthy: 1 / true / yes / on)
EDGION_REQUIRE_FIPS=1 ./edgion-gateway --config-file config/edgion-gateway.yaml
```

### `fips_policy` — force the approved policy on a non-FIPS build

Force-enable the FIPS-approved cipher/curve **policy** on downstream listeners even when the binary
is not a FIPS build. This is a policy switch only — it does **not** provide the validated module.

| Field | Type | Default | Env | Description |
|-------|------|---------|-----|-------------|
| `fips_policy` | `bool` | `false` | `EDGION_FIPS_POLICY` | Force the approved cipher/curve enforcement regardless of the validated module |

- `fips_policy: false` (default) — enforcement follows the build: a real FIPS build enforces the
  approved set automatically; a non-FIPS build does not.
- `fips_policy: true` — the approved cipher/curve set is enforced on a normal BoringSSL build too.

The enforcement gate is effectively `fips_module_available() OR fips_policy`. Two uses:

1. Exercise the FIPS cipher/curve behavior in integration tests without a FIPS toolchain or module
   (the approved-suite restriction is pure SSL config and works on any BoringSSL build).
2. Opt into the approved-suite restriction as defense-in-depth.

> ⚠️ `fips_policy: true` does **not** make a non-FIPS binary FIPS-compliant. For real FIPS
> compliance the binary must be a FIPS build (validated module). This flag only mirrors the
> **cipher/curve policy**.

## Per-listener cipher override

FIPS defaults are per-listener overridable through the existing `EdgionTls.spec.ciphers` field — no
new schema. A listener that must deviate (for example, to interoperate with a client that needs a
specific suite) sets `ciphers` explicitly, and that list wins over the FIPS default:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: custom-ciphers
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_2
  ciphers:
    - ECDHE-ECDSA-AES256-GCM-SHA384
    - ECDHE-RSA-AES256-GCM-SHA384
```

Decision order on a FIPS build:

```text
operator set spec.ciphers   -> use them (override wins)
else FIPS build / fips_policy -> apply the approved default cipher list + curves
else                          -> leave the BoringSSL default (non-FIPS behavior)
```

The approved ECDH curves still apply even when `ciphers` is overridden. See
[EdgionTls](./edgion-tls.md) for the full cipher field semantics.

## Verifying FIPS is active

The Gateway exposes FIPS status through observability, not through a business API:

- **Startup log** — one INFO line reports the module status, a build-time module identifier, and
  whether the approved defaults are active.
- **Metric** — `edgion_gateway_fips_module_enabled` (gauge, `0`/`1`) reflects Layer-1 truth: `1`
  when the validated module is linked. Scrape it via the Gateway metrics endpoint (see
  [Metrics](../../observability/metrics.md)).

To confirm the negotiated suite from a client:

```bash
# Should negotiate an approved TLS 1.2 suite; a non-approved suite (e.g. ChaCha20) is rejected
openssl s_client -connect secure.example.com:443 -tls1_2 \
  -cipher 'ECDHE-RSA-CHACHA20-POLY1305' </dev/null
# -> handshake failure on a FIPS build (ChaCha20 not in the approved TLS 1.2 set)

openssl s_client -connect secure.example.com:443 -tls1_2 \
  -cipher 'ECDHE-RSA-AES256-GCM-SHA384' </dev/null
# -> succeeds
```

## Known limitations

FIPS mode covers the common downstream posture, but has documented gaps (see the full design in
`docs/superpowers/specs/2026-07-02-boringssl-fips-design.md`):

- **TLS 1.3 cipher suites cannot be restricted in BoringSSL.** `TLS_CHACHA20_POLY1305_SHA256`
  remains negotiable at TLS 1.3. Approved-suite enforcement bites at **TLS 1.2 only**. To eliminate
  TLS 1.3 ChaCha20 entirely you would have to disable TLS 1.3 (not done by default).
- **Upstream (Gateway → backend) TLS ciphers are not FIPS-restricted.** The Pingora 0.8.1 connector
  has a static cipher list with no per-peer hook. Upstream relies on the validated module plus
  existing server-certificate verification.
- **No key-type policy** (e.g. rejecting Ed25519 leaf keys) is enforced.
- **TLS 1.3 session resumption (PSK)** skips the per-connection cipher/curve restriction, so a
  resumed TLS 1.3 handshake's ephemeral group is not re-restricted. Consistent with the existing
  per-connection `EdgionTls.ciphers` mechanism and within the TLS 1.3 limitation above.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Gateway exits non-zero at startup with a FIPS error | `require_fips: true` on a non-FIPS image | Deploy a FIPS-build image, or unset `require_fips` if FIPS is not required on this node |
| `edgion_gateway_fips_module_enabled` is `0` | Binary is not a FIPS build | The running image was not compiled with the `fips` feature — use a FIPS image |
| A ChaCha20 client fails on TLS 1.2 but you expected it to work | FIPS approved-default cipher list excludes ChaCha20 | Set `EdgionTls.spec.ciphers` explicitly on that listener, or route those clients to a non-FIPS listener |
| ChaCha20 still negotiates on TLS 1.3 | BoringSSL cannot restrict TLS 1.3 suites (documented limitation) | Pin `minTlsVersion: TLS1_2` and disable TLS 1.3 on that listener if you must exclude it |
| Approved policy not applied on a local (non-FIPS) build | `fips_policy` not set | Set `fips_policy: true` or `EDGION_FIPS_POLICY=1` to exercise the policy without a FIPS module |

## Related documents

- [EdgionTls](./edgion-tls.md) — cipher / minimum-TLS-version control and the `ciphers` override
- [TLS Termination](./tls-termination.md)
- [Gateway Configuration](../../configuration/gateway-config.md) — `require_fips` / `fips_policy` reference
- [Metrics](../../observability/metrics.md) — scraping `edgion_gateway_fips_module_enabled`
