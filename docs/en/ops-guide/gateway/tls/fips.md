# FIPS Mode (BoringSSL)

> **🔒 FIPS-mode build feature**
>
> FIPS mode runs the Gateway data plane on BoringSSL's FIPS-mode crypto module and makes
> downstream (client → Gateway) TLS default to the FIPS-approved cipher and curve set. It is a
> **build/image concern**, not a runtime toggle: the module is compiled in when the
> Gateway image is built with the `fips` cargo feature. There is no per-listener or per-connection
> FIPS switch.

FIPS mode targets deployments that need a FIPS-aligned cryptographic posture. When the Gateway
binary is a FIPS build, two things hold:

1. **FIPS-mode module** — BoringSSL reports FIPS mode (`boring::fips::enabled()` is true) and its
   runtime integrity self-test passes.
2. **Approved-by-default TLS** — downstream server TLS defaults to the FIPS-approved TLS 1.2
   cipher set and ECDH curves, unless an operator sets EGC defaults or an EdgionTls cipher override.

This applies to the **BoringSSL** backend only (the default TLS backend). FIPS is not available on
the `rustls` backend.

> `boring::fips::enabled()`, the module metric, and the integrity self-test prove runtime mode and
> built behavior only. They do not identify a CMVP validated-module stream, certificate scope,
> artifact provenance, or the compliance of a complete deployment. Operators making a compliance
> claim must validate those items separately against their required program and platform.

## What a FIPS build enforces

On a FIPS build, downstream TLS-terminating listeners apply the approved default unless overridden:

| Aspect | FIPS-approved default |
|--------|-----------------------|
| TLS 1.2 cipher list | `ECDHE-ECDSA-AES128-GCM-SHA256`, `ECDHE-RSA-AES128-GCM-SHA256`, `ECDHE-ECDSA-AES256-GCM-SHA384`, `ECDHE-RSA-AES256-GCM-SHA384` (excludes ChaCha20, CBC-SHA1, RC4, 3DES) |
| ECDH curves | `P-256`, `P-384`, `P-521` |
| Minimum TLS version | Unchanged — BoringSSL already defaults to TLS 1.2+, which is FIPS-acceptable |
| TLS 1.3 suites | Unchanged (see [Known limitations](#known-limitations)) |

Cipher precedence is explicit EdgionTls list, then EGC
`spec.downstreamTls.ciphers`, then the approved FIPS default. An explicit `[]`
at either level selects backend cipher defaults. Minimum version uses the same
EdgionTls-over-EGC precedence. Curves remain EGC-only: absent `ecdhCurves` uses
the NIST profile, explicit NIST subsets/order are retained, and X25519 fails
closed. Cipher overrides never relax that curve restriction. See
[downstream TLS defaults](../../configuration/edgion-gateway-config.md).

## Enabling FIPS

FIPS is enabled at **image build time**, not at runtime. The ordinary release workflow, default
workspace build, and local `cargo build` do **not** include FIPS. Build it explicitly through the
repository's Linux multi-architecture builder:

```bash
FEATURES="allocator-jemalloc,fips,full" \
  ./cicd/build-image.sh --arch amd64,arm64 --compile-only
```

The canonical source FIPS build supports `amd64` and `arm64`. Boring 5.4 bundles the upstream
arm64 integrity-hash relocation fix. Ordinary and FIPS builds share the architecture builder and
use the crate's bundled source without a separate checkout or downloaded patch. Ordinary builds
and the release workflow remain non-FIPS. Omit `--compile-only` to package the images. Set
`CARGO_BUILD_JOBS` to optionally cap compilation parallelism inside the build containers.
`BORING_BSSL_FIPS_PATH` remains an upstream prebuilt-tree escape hatch, outside the canonical
Edgion script contract.

When `fips` is selected, the script automatically runs an isolated verification on each release
binary. The Gateway must start with `EDGION_REQUIRE_FIPS=1`, report
`edgion_gateway_fips_module_enabled 1`, reject a ChaCha20-only TLS 1.2 client, and accept an
approved AES-GCM client before the build succeeds. The general integration suite stays on the
ordinary BoringSSL build because its weak-cipher compatibility cases intentionally exercise a
different posture.

There is no config field or environment variable that turns FIPS "on" — you deploy an explicitly
built and verified FIPS image.
The config flags below control **assertions and policy**, not whether BoringSSL FIPS mode is
present.

## Host FIPS detection and the `auto` default

The gateway also detects whether the **host kernel** is in FIPS mode, by reading
`/proc/sys/crypto/fips_enabled` once at startup. BoringSSL is statically linked into the binary, so
the host's FIPS mode has no effect on the crypto executing in the process — this detection cannot
turn a non-FIPS binary into a FIPS-mode one. It drives two independent things instead:

1. **Posture guardrail** — if the host kernel reports FIPS mode but this binary does not run
   BoringSSL in FIPS mode, the gateway emits a startup WARN naming the restricted suites and the escape
   hatch, and starts normally (fail-fast remains opt-in via `require_fips`).
2. **`auto` policy default** — `fips_policy` (below) defaults to `auto`, which enforces the
   approved cipher/curve profile whenever the host is a FIPS node, even on a non-FIPS binary.

**Consequence for operators**: on a FIPS-enabled node, a gateway deployed with **no FIPS
configuration at all** now automatically restricts TLS 1.2 to the four approved AES-GCM cipher
suites and to the P-256/P-384/P-521 curves. A TLS 1.2 client that can only negotiate ChaCha20 or a
CBC suite will fail to handshake. This is a real behavior change driven purely by the node's kernel
flag — no gateway config changed. Escape hatches:

- `fips_policy: off` — disable the forced policy for this deployment (a genuine FIPS build still
  enforces the approved profile unconditionally; `off` only disables the module-less/auto path).
- `EdgionTls.spec.ciphers` — set an explicit cipher list per workload/listener; the operator
  override always wins over any FIPS default.

### The `Unknown` host state

Reading the flag file can end in three states, not just enabled/disabled:

| State | When | Effect |
|-------|------|--------|
| `Enabled` | file readable, contents `1` | `auto` enforces; mismatch guardrail can fire if this binary is non-FIPS |
| `Disabled` | file readable and not `1`, or absent (ENOENT / non-Linux) | `auto` does not enforce |
| `Unknown` | file exists but cannot be read (e.g. a hardened/masked `/proc`) | logs a **WARN**, sets the `edgion_gateway_fips_host_detect_error` gauge to `1`, does **not** fire the posture guardrail, and does **not** tighten ciphers under `auto` |

`Unknown` fails open on traffic (an undetermined host is not evidence of a violation) but is
deliberately loud in logs and metrics, since a hardened node masking `/proc` is exactly the kind of
high-assurance environment this feature targets.

### Known limitation: sandboxed container runtimes

Under VM- or sandbox-isolated container runtimes — Kata Containers, Firecracker microVMs, gVisor —
`/proc/sys/crypto/fips_enabled` reflects the **guest/sandbox kernel**, not the physical node. The
read *succeeds* and returns a valid-but-wrong value, so no detection state or log line can catch
this case. Under the default namespace-sharing runtimes (runc/containerd) the file correctly
reflects the host kernel.

**Operators on a sandboxed runtime must set `fips_policy` explicitly** (`on` or `off`) rather than
relying on `auto`.

## Configuration

Two gateway-local (process, non-cluster-synced) startup flags govern FIPS behavior. Both live in the
Gateway YAML config and have environment-variable equivalents. See the full
[Gateway Configuration](../../configuration/gateway-config.md) reference.

### `require_fips` — fail-closed startup guard

Assert at startup that the running binary is a FIPS build. Use this on nodes that **must** be FIPS
to catch a misconfigured (non-FIPS) image before it serves traffic.

| Field | Type | Default | Env | Description |
|-------|------|---------|-----|-------------|
| `require_fips` | `bool` | `false` | `EDGION_REQUIRE_FIPS` | Require BoringSSL FIPS mode to be present at startup |

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

### `fips_policy` — select the approved policy regardless of FIPS mode

A tri-state that selects the FIPS-approved cipher/curve **policy** on downstream listeners,
independent of whether the binary is a FIPS build. This is a policy switch only — it does **not**
provide BoringSSL FIPS mode.

| Field | Type | Default | Env | Description |
|-------|------|---------|-----|-------------|
| `fips_policy` | `"auto" \| "on" \| "off"` | `auto` | `EDGION_FIPS_POLICY` | Select the approved cipher/curve enforcement regardless of FIPS mode |

- `auto` (default) — follow the host: enforces the approved set when [host FIPS
  detection](#host-fips-detection-and-the-auto-default) reports the host kernel is in FIPS mode;
  otherwise does not. Cipher-suite policy alignment only, not a compliance claim.
- `on` — the approved cipher/curve set is enforced on a normal BoringSSL build too, regardless of
  the host.
- `off` — never force-enables the policy. A genuine FIPS build still enforces the approved profile
  unconditionally; `off` only disables the module-less (forced) path.

The enforcement gate is effectively `fips_module_available() OR fips_policy_forced`, where
`fips_policy_forced` comes from resolving this setting (and, for `auto`, the host state). Two uses
for `on`:

1. Exercise the FIPS cipher/curve behavior in integration tests without a FIPS toolchain or module
   (the approved-suite restriction is pure SSL config and works on any BoringSSL build).
2. Opt into the approved-suite restriction as defense-in-depth.

> ⚠️ `fips_policy: on` (or the `auto` default tightening on a FIPS host) does **not** make a
> non-FIPS binary FIPS-compliant. A FIPS-mode binary is necessary for this Edgion posture but is
> not, by itself, proof of CMVP certificate scope, artifact provenance, platform validation, or
> deployment compliance. This field only mirrors the **cipher/curve policy**.

**Accepted values and precedence.** Only `auto`/`on`/`off` are accepted — there are no legacy
booleans and no legacy truthy/falsey tokens. A YAML boolean (`fips_policy: true`) is a hard
deserialize error, and YAML 1.1 reads a bare `on` / `off` scalar as a boolean, so those two values
must be quoted in the config file:

```yaml
# edgion-gateway.yaml
fips_policy: "off"   # or "on"; `auto` needs no quotes
```

Resolution order, highest first: **env → CLI → config file → `auto`**.

```bash
# env accepts only auto/on/off; anything else (including the old 1/true/0/false)
# is an unrecognized token that WARNs and is ignored, falling through to CLI/file
EDGION_FIPS_POLICY=off ./edgion-gateway --config-file config/edgion-gateway.yaml

# CLI: bare flag means "on"; --fips-policy=auto / --fips-policy=off also work
./edgion-gateway --fips-policy --config-file config/edgion-gateway.yaml
```

`--fips-policy` and `--require-fips` **now take effect** on the CLI: both were previously parsed by
clap and then silently discarded by the config merge, so only YAML and env reached the running
gateway. That merge bug is fixed — the CLI value is honored when the flag is passed, and omitting
the flag leaves any config-file value intact.

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

Decision order:

```text
EdgionTls.spec.ciphers is set    -> use that list; [] selects backend defaults
else EGC downstreamTls.ciphers is set -> use that list; [] selects backend defaults
else fips_policy_active()        -> apply the approved default cipher list
  (true on a FIPS build, OR on fips_policy=on, OR on fips_policy=auto with a FIPS host)
else                              -> leave the BoringSSL default (non-FIPS behavior)
```

Curve restrictions still apply even when `ciphers` is overridden. A downstream
ECDH configuration may select a subset of `P-256`, `P-384`, `P-521`; a list
containing `X25519` rejects new handshakes while FIPS policy is active. See
[ECDH curve policy](./tls-termination.md#ecdh-curve-policy) for the EGC configuration. See
[EdgionTls](./edgion-tls.md) for the full cipher field semantics.

## Verifying FIPS is active

The Gateway exposes FIPS status through observability, not through a business API:

- **Startup log** — one INFO line (`"FIPS status resolved"`) reports the module status, the
  detected host FIPS state, the resolved `fips_policy` mode, and whether the approved profile is
  active.
- **Metrics** — four gauges, all set unconditionally at startup (never lazily registered, so an
  absent series never masks a `== 0` condition):

  | Gauge | Meaning |
  |-------|---------|
  | `edgion_gateway_fips_module_enabled` | `1` when BoringSSL reports FIPS mode in this binary. This proves mode, not CMVP certificate scope or build provenance. |
  | `edgion_gateway_fips_host_enabled` | `1` when the host kernel reports FIPS mode (`/proc/sys/crypto/fips_enabled`) |
  | `edgion_gateway_fips_host_detect_error` | `1` when host detection could not read the flag file (`HostFipsState::Unknown`) — the guardrail and the `auto` default are both inert while this is `1` |
  | `edgion_gateway_fips_policy_active{module_enabled="true\|false"}` | `1` when the approved cipher/curve profile is actually enforced on this process; the label records the BoringSSL FIPS-mode signal |

  Scrape them via the Gateway metrics endpoint (see [Metrics](../../observability/metrics.md)).

> ⚠️ **`fips_policy_active{module_enabled="false"}` is not a compliance pass.** It means the approved
> cipher suites are being presented without BoringSSL FIPS mode — exactly what
> `auto`/`on` produce on a non-FIPS binary. A dashboard tile or an external cipher-suite scanner that
> only checks the negotiated suite will see the same approved suites a real FIPS build presents and
> may read this as "compliant". It is not. The module metric distinguishes the two runtime modes,
> but neither value proves certificate scope, provenance, or deployment compliance.

### Posture alert

The host-vs-binary posture mismatch — a FIPS node running a binary without BoringSSL FIPS mode — is
one PromQL expression:

```promql
edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0
```

This is exactly the condition the startup-time guardrail WARN also checks; the metric lets you
alert on it fleet-wide instead of grepping pod logs.

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

FIPS mode covers the common downstream posture, but has documented gaps (see the current contract in
`skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md`):

- **The current Gateway cipher-list policy does not restrict TLS 1.3 suites.** `TLS_CHACHA20_POLY1305_SHA256`
  remains negotiable at TLS 1.3. Approved-suite enforcement bites at **TLS 1.2 only**. To eliminate
  TLS 1.3 ChaCha20 entirely you would have to disable TLS 1.3 (not done by default).
- **Upstream (Gateway → backend) TLS ciphers are not FIPS-restricted.** The Pingora 0.8.1 connector
  has a static cipher list with no per-peer hook. Upstream relies on BoringSSL FIPS mode plus
  existing server-certificate verification.
- **No key-type policy** (e.g. rejecting Ed25519 leaf keys) is enforced.
- **TLS 1.3 session resumption (PSK)** has the same cipher-suite limitation as a full TLS 1.3
  handshake: the Gateway's TLS 1.2 cipher-list API does not restrict TLS 1.3 suites. Curve policy is applied
  separately in the early ClientHello callback, before certificate lookup. PSK-specific curve
  negotiation has not been verified by this change.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Gateway exits non-zero at startup with a FIPS error | `require_fips: true` on a non-FIPS image | Deploy a FIPS-build image, or unset `require_fips` if FIPS is not required on this node |
| `edgion_gateway_fips_module_enabled` is `0` | Binary is not a FIPS build | The running image was not compiled with the `fips` feature — use a FIPS image |
| A ChaCha20 or CBC-only TLS 1.2 client suddenly fails after no gateway config change | The node became a FIPS host and `fips_policy` is at its `auto` default | Set `fips_policy: off` for this deployment, or `EdgionTls.spec.ciphers` for this workload |
| A ChaCha20 client fails on TLS 1.2 but you expected it to work | FIPS approved-default cipher list excludes ChaCha20 | Set `EdgionTls.spec.ciphers` explicitly on that listener, or route those clients to a non-FIPS listener |
| ChaCha20 still negotiates on TLS 1.3 | The current Gateway cipher-list policy only controls TLS 1.2 suites | Excluding TLS 1.3 ChaCha20 requires a separate TLS policy change |
| Approved policy not applied on a local (non-FIPS) build / non-FIPS host | `fips_policy` resolves to `off` (or `auto` with a non-FIPS host) | Set `fips_policy: on` (or `EDGION_FIPS_POLICY=on`) to exercise the policy without a FIPS module |
| `fips_policy: true` in YAML fails to load | Legacy boolean spelling; `fips_policy` is a string tri-state, not a bool | Use `fips_policy: "on"` / `"off"` / `auto` |
| `edgion_gateway_fips_host_detect_error` is `1` | Host detection could not read `/proc/sys/crypto/fips_enabled` (e.g. hardened/masked `/proc`) | Set `fips_policy` explicitly (`on`/`off`); `auto` is inert while detection is `Unknown` |
| Node is a FIPS host but `edgion_gateway_fips_host_enabled` reads `0` (or vice versa) inside a sandboxed runtime | Kata/Firecracker/gVisor: `/proc/sys/crypto/fips_enabled` reflects the guest kernel, not the physical node | Set `fips_policy` explicitly on that runtime; do not rely on `auto` |

## Related documents

- [EdgionTls](./edgion-tls.md) — cipher / minimum-TLS-version control and the `ciphers` override
- [TLS Termination](./tls-termination.md)
- [Gateway Configuration](../../configuration/gateway-config.md) — `require_fips` / `fips_policy` reference
- [Metrics](../../observability/metrics.md) — scraping `edgion_gateway_fips_module_enabled`, `edgion_gateway_fips_host_enabled`, `edgion_gateway_fips_host_detect_error`, `edgion_gateway_fips_policy_active`
