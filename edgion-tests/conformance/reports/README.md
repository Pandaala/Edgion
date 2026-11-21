# Gateway API Conformance Test Reports

The 2026-06 reports are historical Edgion `dev` runs. They record
`gatewayAPIVersion: UNDEFINED` and `gatewayAPIChannel: UNDEFINED`, so they
cannot establish compatibility with a particular installed CRD bundle.
The current Go module pins both `sigs.k8s.io/gateway-api` and
`sigs.k8s.io/gateway-api/conformance` at v1.6.2. These updated dependencies do
not recover the missing bundle/channel metadata from an old run.

## Latest Standard v1.6.2 rerun — 2026-10-11

A fresh full-profile rerun used checkout `20cfc93e391f4b29a2466a6ad9afe5adae1c3a93`
on `kind-edgion-standard-162`. Product source remains `6f566a8ae5e066a9b780f19022143db64a3eb2b4`;
the verified Linux arm64 images and unchanged runner from the initial run below
were reused, with no new build or feature narrowing.

| Profile | Core | Extended | Fresh report (UTC) |
| --- | --- | --- | --- |
| HTTP | 37 passed | 30 passed | [HTTP/GRPC, 03:21:34](gateway-http-grpc-v1.6.2-standard-kind-2026-10-11-rerun.yaml) |
| GRPC | 15 passed | Not reported | [HTTP/GRPC, 03:21:34](gateway-http-grpc-v1.6.2-standard-kind-2026-10-11-rerun.yaml) |
| TLS | 20 passed | Not reported | [TLS, 03:23:53](gateway-tls-v1.6.2-standard-kind-2026-10-11-rerun.yaml) |

Each reported profile has zero failures/skips. Both stage exits and explicit
report-copy exits were zero. Raw top-level results were 71 HTTP/GRPC passes and
20 TLS passes, with respectively 82 and 133 unsupported or out-of-profile skips.
Eleven passing identities overlap between stages: 80 distinct identities passed;
profile counts must not be added as unique test counts.

Actual image config/layer identities matched the verified builds; all 19 CRD
identities/spec hashes stayed unchanged. All 28 backend Deployment/Service UIDs
were retained. No test Route, Grant, policy or dedicated pair remained; the two
adopted TLS fixture names, Secrets and runner were preserved. Final cleanup
needed no mutation. An initial readiness check observed an old Controller Pod
still terminating; fresh verification passed after normal deletion, before tests.

The raw reports are copied byte for byte. Logs, hashes and provenance were kept in
local `tmp/resource-reference-contract/conformance-repeat-20cfc93e-20261011/execution-manifest.json`
evidence, which is no longer retained.
This result supplies no Kubernetes CredentialInjector dynamic/ACK evidence or
broader custom-reference acceptance. The separate F2-B Kubernetes execution
followed later: `CredentialInjectorDynamic` passed on Kubernetes on 2026-10-27
(source `5e359f300`); see the
[resource reference contract evidence](../../../skills/01-architecture/00-common/06-resource-reference-contract.md#verification-evidence).

## Initial Standard v1.6.2 run — 2026-10-11

Product source: `6f566a8ae5e066a9b780f19022143db64a3eb2b4`. Fresh canonical
Linux arm64 release Controller and default-BoringSSL Gateway images ran on
`kind-edgion-standard-162`, with Gateway API Standard v1.6.2 and current Edgion
CRDs. All 19 installed CRD version schemas, served/storage flags and subresources
were checked against that source. Schema comparison is semantic: equivalent
JSON numeric bounds such as `0` and `0.0` need not have identical byte hashes.

| Profile | Core | Extended | Report |
| --- | --- | --- | --- |
| HTTP | 37 passed, 0 failed/skipped | 30 passed, 0 failed/skipped | [HTTP/GRPC report](gateway-http-grpc-v1.6.2-standard-kind-2026-10-11.yaml) |
| GRPC | 15 passed, 0 failed/skipped | Not reported | [HTTP/GRPC report](gateway-http-grpc-v1.6.2-standard-kind-2026-10-11.yaml) |
| TLS | 20 passed, 0 failed/skipped | Not reported | [TLS report](gateway-tls-v1.6.2-standard-kind-2026-10-11.yaml) |

Both invocations exited zero without narrowing the canonical declared features.
HTTP/GRPC ran 71 unique passing top-level cases; TLS ran 20. The respective
82 and 133 unsupported or out-of-profile cases were skipped by the framework.
Profile counts overlap and must not be added as unique test counts.

The HTTP/GRPC phase used `base-manifests-http-grpc-scoped.yaml` and two
UID-scoped dedicated Gateway instances. The separate TLS phase used
`base-manifests-tls-only.yaml` with passthrough listeners on port 443. Both used
the same freshly compiled Go conformance v1.6.2 runner, with identical local and
in-pod SHA-256 `fd57e13ff87435e30c9038ed441a60f832d226849cb892c4a6d3d8908eb1026d`.
The reported implementation version remains `dev`; this is a source-specific
local verification result, not a release certification or support expansion.

The Controller and Gateway image tags were `pandaala/edgion-controller:f2-6f566a8ae5e0`
and `pandaala/edgion-gateway:f2-6f566a8ae5e0`. Runtime image config digests and full
layer identities matched the recorded builds. After execution, no test Routes,
ReferenceGrants, BackendTLSPolicies or dedicated instances remained. Existing
backends, Secrets, runner and two pre-existing TLS fixture names were retained.
Raw logs, image/schema provenance and independent reviews were kept locally
under `tmp/resource-reference-contract/f2-run-6f566a8ae5e066a9b780f19022143db64a3eb2b4/`,
which is no longer retained. Task-specific reference admission, compatibility and
dynamic traffic matrices were separate acceptance work (see the
[resource reference contract evidence](../../../skills/01-architecture/00-common/06-resource-reference-contract.md#verification-evidence)); these profiles do not establish that
coverage.

## Why Split-Profile Runs

Edgion does **not** support HTTPS Terminate and TLS Passthrough on the same port.
The upstream conformance tests hardcode port 443 for both modes. Running all
profiles together causes TLS Passthrough requests to be intercepted by the HTTPS
Terminate listener, producing certificate mismatches and test failures.

Therefore GATEWAY-HTTP/GRPC and GATEWAY-TLS **must be run separately**, each
with its own base manifest set and gateway startup configuration:

- **HTTP/GRPC run**: uses `base-manifests-http-grpc-scoped.yaml` (HTTP Gateways on 80/443 with UID-scoped dedicated instances)
- **TLS run**: uses `base-manifests-tls-only.yaml` (HTTP Gateways removed) +
  `conformance-gateway-tls.yaml` (TLS pre-bind Gateways on 443/8443/8883)

## Report Files

| File | Profiles | Date | Notes |
|------|----------|------|-------|
| `gateway-http-grpc-conformance-report.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-06-18 13:54:50 UTC | Split-profile run |
| `gateway-tls-conformance-report.yaml` | GATEWAY-TLS | 2026-06-18 13:17:59 UTC | Split-profile run with TLS-only base manifest |
| `gateway-http-conformance-report.yaml` | GATEWAY-HTTP | 2026-05-23 | Earlier partial/failing report, retained as history |
| `gateway-http-grpc-v1.6.2-kind-2026-09-23.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-09-23 | v1.6.2 Experimental, isolated kind node run; all selected tests passed |
| `gateway-tls-v1.6.2-kind-2026-09-23.yaml` | GATEWAY-TLS | 2026-09-23 | v1.6.2 Experimental, isolated kind node run; one extended test failed |
| `gateway-http-grpc-v1.6.2-standard-kind-2026-09-24.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-09-24 | v1.6.2 Standard; HTTP Core 36/37, Extended 26/30, GRPC Core 15/15 |
| `gateway-tls-v1.6.2-standard-kind-2026-09-24.yaml` | GATEWAY-TLS | 2026-09-24 | v1.6.2 Standard; TLS Core 20/20 |
| `gateway-http-grpc-v1.6.2-standard-kind-2026-10-11.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-10-11 | Current source run; HTTP Core 37/37, Extended 30/30, GRPC Core 15/15 |
| `gateway-tls-v1.6.2-standard-kind-2026-10-11.yaml` | GATEWAY-TLS | 2026-10-11 | Current source run; TLS Core 20/20 |
| `gateway-http-grpc-v1.6.2-standard-kind-2026-10-11-rerun.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-10-11 03:21:34 UTC | Fresh rerun with verified existing binaries; HTTP Core 37/37, Extended 30/30, GRPC Core 15/15 |
| `gateway-tls-v1.6.2-standard-kind-2026-10-11-rerun.yaml` | GATEWAY-TLS | 2026-10-11 03:23:53 UTC | Fresh rerun with the same verified runner; TLS Core 20/20 |

## Historical Standard v1.6.2 result boundary — 2026-09-24

The Standard runs used an isolated kind cluster on OrbStack and local arm64
Controller and Gateway images. The unmodified upstream HTTP/GRPC suite failed
`HTTPRouteMultipleGateways`: the four base Gateways all published the same
Gateway Service IP, so identical host/path requests could not select a
distinct Gateway parent. Four HTTP Extended tests also failed in the combined
run; a targeted `BackendTLSPolicySANValidation` run passed, but that does not
establish a full-profile pass. GRPC Core passed 15/15.

The TLS-only run used the unmodified upstream test binary with the pinned
TLS-only base manifest and passed Core 20/20. It does not report an Extended
profile. TCP and UDP remain outside the declared feature set because dynamic
listener binding is not implemented.

An addressed-hostNetwork experiment was excluded from these reports: it
assigned node loopback IPs to the test Gateways and patched upstream fixture
preparation to preserve them. Normal Gateway Pods use their own network
namespace behind a Service, so a Gateway address or Service VIP need not be
present on a Pod. That experiment does not establish support for binding
`Gateway.spec.addresses` in the default Kubernetes deployment.

The 2026-09-23 Experimental reports use Go conformance v1.6.2 and Gateway API v1.6.2
Experimental CRDs with local arm64 Edgion images. The full HTTP profile
passed Core 37/37 and Extended 30/30; GRPC passed Core 15/15. The full TLS
profile passed Core 20/20 and Extended 2/3. Its only failure was
`TLSRouteTerminateSimpleSameNamespace` after a separate targeted pass. The TLS
full-profile follow-up was canceled; `TLSRouteModeTerminate` remains unsupported.
The TCP and UDP full profiles timed out while probing listener ports created
after Gateway startup, so neither produced a report. Dynamic TCP/UDP listener
support was declined; port changes require a Gateway process restart. See the
[accepted listener boundary](../../../skills/04-review/listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md).

## Historical run commands

```bash
# Step 1: HTTP + gRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m

# Step 2: TLS (requires separate run — port 443 conflict)
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
```

These commands describe the historical split-profile setup. A new run uses
the then-current code and installed CRDs; it cannot recreate the undefined
bundle/channel metadata in the older reports.

## Results Summary

### GATEWAY-HTTP — 2026-06-18 report

- **Core**: 33/33 passed
- **Extended**: 31/31 passed

### GATEWAY-GRPC — 2026-06-18 report

- **Core**: 13/13 passed

### GATEWAY-TLS — 2026-06-18 report

- **Core**: 18/18 passed
- **Extended**: 3/3 passed (`TLSRouteModeTerminate`, `GatewayHTTPListenerIsolation`, `GatewayHTTPSListenerDetectMisdirectedRequests`)

The earlier GATEWAY-HTTP report dated 2026-05-23 recorded core 0 passed,
33 skipped, and extended 0 passed, 1 failed, 31 skipped. It is retained so
the later improvement can be audited. Neither report series has a defined
installed bundle version or channel, and no test result here is assigned to
a specific source GVK or release in the offline fixture matrix.

## Known Quirks

### Historical certificate issue

An earlier Go toolchain and conformance suite combination exposed a self-signed
certificate CA constraint issue. The current v1.6.2 module has no local
conformance replacement. The historical June reports predate this module update.

### TLS backend pods must restart after cert regeneration

TLS backend pods (`tls-backend`, `tls-backend-2`, `tcp-backend`) mount certificate
Secrets as volumes and load them once at startup. When the conformance framework
regenerates Secrets with new key pairs, running pods still serve old certificates.
`--clean-slate` handles this automatically by rolling-restarting all TLS backend
deployments before running tests.
