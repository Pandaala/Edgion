---
name: mandatory-integration-gates-fail-closed
description: Use when reviewing integration validation prerequisites, complete response observations, skips, or infrastructure failures.
knowledge_type: regression-guards
---

# Mandatory integration gates fail closed

A required integration validation is successful only when it actually runs and proves its
invariant. An unavailable metrics or diagnostics endpoint, missing authentication material,
unreadable required log, missing validator, or validator infrastructure error is a failed gate,
not a skip or pass.

Skips require an explicit operator-facing opt-out and must be recorded with its reason. The main
runner currently permits `--skip-residue-check` (or `SKIP_RESIDUE_CHECK=true`) for cache-residue
validation and `RESIDUE_MODE=warn` for a deliberately non-gating residue policy. Missing
prerequisites never activate those policies implicitly.

The cache-residue validator also treats the store-stats response envelope as a
mandatory prerequisite. Both Controller and Gateway responses must contain
`success: true` and an object-valued `data` field before any counts are walked.
An HTTP 200 response with `success: false`, a missing/non-boolean `success`, or
missing/null/non-object `data` is an infrastructure error, never an empty cache
snapshot. This prevents an unavailable or contract-broken diagnostics handler
from making the final residue gate pass without checking either process.

## Configuration submission and log pipelines

`start_all_with_conf.sh::load_configs` submits each selected suite through
`load_conf.sh` and captures its output with `tee`. Both processes must succeed:
without `pipefail`, a rejected YAML batch is hidden by a successful log write,
leaving `failed=false` and allowing startup to proceed with incomplete fixtures.
The loader now scopes `pipefail` to that pipeline's subshell, preserving caller
options and the remaining script's pipeline semantics. Failures identify the
suite and log path; remaining suites are attempted, then the aggregate failure
aborts startup before Gateway tests run. Log sink failures are also fatal.

The original `edgionplugins-resource-sweep-config-list-drift` task described
missing fixture inventories and historical HTTP 404 failures. The corrected
resource sweep has directory coverage for every executed item and rejects
explicit preload entries that do not name a real fixture directory.
`PluginCondition/AllConditions` is covered recursively by its parent, and the
logical `DslGrpcError` execution item is covered by `EdgionPlugins/Dsl` rather
than a nonexistent same-named directory. Keep execution registration evidence in the
[suite-registration guard](integration-suite-registrations-must-be-checked-per-sweep.md).

Live verification on 2026-09-15 loaded the complete EdgionPlugins inventory;
every executed plugin suite passed, including the historically failing suites
and `DslGrpcError`; the metrics-label, unknown-field, and requeue final gates
also passed. The report recorded 64 PASS entries, one SKIP, and one final
cache-residue failure caused by a distinct Wasm delayed-eviction deadline
mismatch. The validator deadline was raised above the then-current 60-second
module eviction grace (subsequently removed by plugin ownership); that historical result does not
invalidate configuration-submission or suite behavior evidence. A focused
`DslGrpcError` run subsequently passed its ordinary cache-residue gate as well.

Verification: `bash cicd/checks/tests/integration_config_load_test.sh` passes
success, loader exit 7 with a successful log sink, log sink exit 9 with a
successful loader, continued collection after one failed suite, and caller
shell-option isolation. These isolated checks use the actual loader function
with temporary fixtures; they do not prove live Controller/Gateway integration.
Re-evaluate when configuration submission, output capture, or startup error
propagation changes.

## Complete response observations before negative assertions

ARCH-348 found twelve response reads in six plugin suites that replaced a failed
`text().await` with an empty string. Their subsequent absence checks could pass
without observing the response. The shared CredentialInjector assertion serves
both the static missing-Secret case and the dynamic deleted-Secret phase, giving
thirteen case consumers. An expected HTTP status does not prove body completion.

The assertion owners remain private to the existing suites under
`edgion-tests/src/client/suites/edgion_plugins/`: `canary/canary.rs`,
`credential_injector/credential_injector.rs`, `ext_proc/ext_proc.rs`,
`forward_auth/forward_auth.rs`, `hmac_auth/hmac_auth.rs`, and `key_auth/key_auth.rs`.
Their selected cases pass the actual response to a feature-local observer that
checks status, awaits the complete body, and only then evaluates its evidence.
Read errors fail with fixed operation diagnostics; neither those diagnostics nor
the touched request/assertion branches print body values or dependency errors.

- The two Canary anti-spoof cases require backend port 30001, GET `/echo`, and
  the echo header section before their existing forged-header absence check.
- ExtProc's dead-processor fail-open case requires the same backend, GET `/get`,
  and the header section before its processor-header absence check.
- CredentialInjector's shared fail-closed assertion and the five selected ExtProc
  sanitized-500 cases accept successfully completed empty bodies. They preserve
  their existing prohibited-marker and generic-body predicates. A failed read is
  distinct from that legitimate empty-body outcome; production fail-close paths
  may deliberately return `body: None`.
- ForwardAuth's strict `[204]` case requires the relayed HTTP 200 and parsed
  authored auth JSON with `status: "ok"` and `user: "test-user"`, while retaining
  backend exclusion. An unrelated 200 is no longer an alternate success path.
- HmacAuth and KeyAuth's selected hide-credential cases require JSON containing
  an object-valued `headers` member. Their existing negative predicates remain:
  HmacAuth rejects authorization text; KeyAuth rejects the conjunction of its
  header-name and authored key markers. This does not tighten KeyAuth policy or
  replace the separate authenticated/anonymous identity assertions.

Each suite's `observation_tests` module exercises the actual response observer
using in-memory `http::Response` conversions and fallible reqwest body streams.
The tests distinguish successful empty 500, invalid or unrelated success bodies,
valid fixture evidence, existing prohibited markers, and body errors both before
and after otherwise-valid bytes. Exact fixed-error checks protect diagnostic
redaction. These tests are network-free response-boundary checks; they do not
prove HTTP transport, credential stripping, live routing, or dynamic convergence.
The static CredentialInjector suite and registered `CredentialInjectorDynamic`
scenario remain responsible for exercising the two actual request callers and
the latter's existing phase barriers and restoration. Local and Kubernetes
fixtures, registrations, case identities, deadlines, and tier placement remain
unchanged. Fresh command results and live execution limits belong in the scoped
implementation verification record; historical passes elsewhere are not evidence
that these observations ran.

Re-evaluate when a negative body assertion changes, response evidence or test
producer format changes, or error-body policy changes. Avoid replacing failed
observations with default data or requiring nonempty bodies for every status.
No shared response framework or production response-body owner is implied.

## Native gRPC kind-denial evidence

ARCH-338 applies the prerequisite rule to the existing
`Gateway/AllowedRoutes/Kinds` negative case. A failed connection or arbitrary RPC
error does not establish that `allowedRoutes.kinds` denied GRPCRoute. The private
owner in `edgion-tests/src/client/suites/gateway/allowed_routes/kinds.rs` uses the
existing RawH2Conn to send the original SayHello request with `KindsTest` and
authority `kinds-grpc.example.com:31213`. Every connection, request send and
response-future error fails the case with a fixed operation diagnostic.

The selected fixture admits only HTTPRoute and has an empty native gRPC table.
Its native request falls through to HTTP lookup; the gRPC-aware HTTP lookup miss
returns HTTP 200, exactly one `content-type: application/grpc`, and exactly one
`grpc-status: 5`. Status12 describes a different populated-native-table miss and
is not accepted here. Missing, duplicate, malformed or unrelated metadata fails.
Successful backend responses and unavailable-backend errors are not alternate
denial outcomes.

The response's typed h2 receive state must also report completed EOS with no
pending DATA or trailers. No body is collected or defaulted after a failed read.
Locked h2 0.4.20 preserves an already-received EOS across a later send-half reset
(`ErrorAfterEndStream`; `proto/streams/state.rs` is unchanged from 0.4.19, checked
by source diff on 2026-11-06); that later event does not invalidate the completed empty
response. This assertion proves receive completion, not rejection of every event
after completion or byte-level frame fragmentation. RawH2Conn remains alive
through observation and its existing Drop aborts its driver on success, failure
or cancellation by the unchanged thirty-second TestCase deadline.

The private `outcome_tests` module exercises the actual typed predicate without
network or servers: exact valid completion, incomplete receive state, wrong HTTP
status, missing/wrong/binary/duplicate content-type, and missing/malformed/binary/
duplicate/conflicting/unrelated gRPC status. It also preserves both original
integration case identities. These regressions establish outcome discrimination;
they do not execute connection refusal, timeouts, reset schedules, live publication
or Kubernetes behavior. Fresh live and fault-run evidence belongs in the scoped
implementation verification record. Fixtures, positive HTTP case, shared topology
and existing suite registration remain unchanged.

A genuine no-route envelope does not uniquely identify kind exclusion: missing
publication or another routing defect can produce the same response. This bounded
correction closes the generic-error false positives without claiming that stronger
causal proof. Re-evaluate when the selected fixture's native-table membership,
HTTP fallback mapping, response contract or h2 receive-EOS semantics change.

## Negative TLS policy evidence

ARCH-283 applies the same prerequisite rule to EdgionTls/mTLS,
EdgionTls/mtls_failclose and Gateway/FrontendValidation. Missing certificate/key
files, malformed or expired identities, mismatched keys, DNS/connect failure,
operation deadline, unrelated TLS errors and an unexpectedly completed handshake
fail the case. A client build error or arbitrary request error never establishes
Gateway rejection.

The scoped test owner is
`edgion-tests/src/client/suites/edgion_tls/mtls_failclose/probe.rs`. It uses the
existing rustls client dependencies, pins the expected serving certificate and
verifies handshake signatures, disables resumption, and forces TLS 1.2 so the
client-authentication decision is terminal when connect returns. Ordinary positive
HTTP cases keep normal TLS negotiation and complete status assertions. The
negative cases require exact typed peer alerts plus successful policy-isolation
controls: valid-client acceptance on the selected target, UnknownIssuer as the
untrusted identity's independent preparation result, and trusted SAN-mismatch
acceptance on ordinary Mutual before rejection on the SAN-restricted target.

Normal mTLS/frontend alert assertions also require this exact attempt to have
verified the pinned certificate and the server handshake signature. Matching
public certificate bytes alone does not establish server key possession. The
prepared probe is consumed by its run, so authentication evidence cannot leak
between attempts. EOF/reset after certificate validation cannot be accepted as
a frontend pre-certificate closure. The existing listener-status predicate
remains the single owner of the required condition conjunction.

The bounded operation drops its owned socket/state on deadline or caller
cancellation, with a network-free drop regression. An already-running blocking
system DNS lookup may outlive caller cancellation because Tokio delegates it to
spawn_blocking; the deadline bounds the caller, not cancellation of that worker.

For the current BoringSSL dependency, the isolated SAN rejection returns exactly
`InternalError`: Edgion's leaf callback returns zero without setting X509 error;
BoringSSL `X509_verify_cert` then changes the failed `X509_V_OK` result to
`X509_V_ERR_UNSPECIFIED`, which `SSL_alert_from_verify_result` maps to
`SSL_AD_INTERNAL_ERROR`. `CertificateUnknown` is not accepted for this case.
The same-identity ordinary-Mutual control, valid-client restricted-SNI control,
independent trust/SAN checks and serving-certificate pin remain required; a bare
InternalError never establishes SAN denial. Missing-certificate and untrusted
cases still accept only HandshakeFailure and UnknownCA respectively. The first
local ARCH-283 run correctly failed an incorrect CertificateUnknown expectation
(10/11); its target-specific audit showed the SAN-rejection prefix without
handshake completion. The corrected source trace accounts for BoringSSL's outer
verification safety net, rather than treating the alert as a generic exception.

Fail-close probes additionally require exact target/control resolved attachment
publication at both Controller and Gateway, and a Terminate control on the same
listener with the same serving Secret. Local runs require appended target-specific
`mtls config:` audit evidence with a connection-time fence; the timestamp excludes
old events still queued by the asynchronous logger. Kubernetes uses wire,
publication and fixture isolation, without assuming a mounted host ssl.log or
claiming direct callback-cause audit. The frontend connection gate runs before
TLS: prompt peer closure after a successful TCP connect is meaningful only with
the exact selected listener's invalid-CA conditions. Refusal and timeout fail.

Probe pins must follow the actual test-client asset projection. Local fail-close
probes use `edgion-tests/certs/server.crt`; Kubernetes explicitly selects
`edgion-tests/certs/ca.pem`, the existing edge-tls Secret's mounted serving leaf.
Do not choose an alternate file after a preparation or pin-verification failure.
The existing Kubernetes mtls ConfigMap must include the public fv-server.crt used
by fv-server-cert as well as mtls-server.crt; no new frontend private key belongs
in that client projection. The isolated
`k8s/scripts/common/tests/negative_tls_probe_assets_test.sh` regression runs the
real generator, reconstructs the actual Deployment's Secret/ConfigMap mount map,
and compares all three pin assets to the corresponding serving Secret bytes.
It also rejects an intentionally missing frontend pin. This establishes generated
asset delivery, not live Kubernetes wire execution. The first local-only
verification missed this introduced mount regression; the asset correction
preserves the same exact pin policy in both topologies.

Network-free regressions cover preparation, typed decisions, per-attempt
signature evidence, deadlines/cancellation, exact listener-condition ownership,
fixture isolation and audit freshness. These tests
do not establish live BoringSSL or Kubernetes behavior; the selected existing
shared-topology suites remain the wire gates. ARCH-283 implementation verification
is recorded separately; no live result is inferred from this design description.
Re-evaluate when supported client/Gateway TLS dependencies, alert mapping, fixture
policies, attachment publication, audit timestamps or listener gates change.

## Negative raw-TCP policy evidence

ARCH-341 applies the prerequisite rule to Gateway/StreamPlugins. Its three
negative cases share the private `observe_rejections` owner in
`edgion-tests/src/client/suites/gateway/stream_plugins/stream_plugins.rs`.
Each attempt must establish the selected TCP connection, then observe prompt
EOF or read-side `ConnectionReset` without application data. Every establishment
error (including reset), connect deadline, other read error, application byte or
established idle deadline fails. Data followed by EOF/reset still fails; timeout
and arbitrary IO failure cannot stand in for policy evidence. All five repeated
attempts run even after a failed attempt, and all five must pass.

The two former negative-probe writes were removed: Stage1 denies after accept
and before application processing, so no payload is required to elicit closure.
Reading directly also avoids ignored/unbounded write errors and write-induced
reset ambiguity hiding already queued application bytes. The allowed bare-name
control retains its existing write and application byte-flow assertion.

The HTTP control (31270), bare-name allow (31269), deny-all (31271) and invalid
qualified-reference (31272) targets remain distinct. A healthy positive target
cannot establish a negative target's availability. Single probes retain 3s
connect/2s close deadlines, repeated probes 2s/1s per attempt, and the existing
30s case owner remains authoritative. Successful messages describe accepted
connection closure, not unique attribution to a plugin: later routing/backend
closure remains a limitation. No production filter, reference contract,
publication, fixture topology, transport framework or telemetry change is implied.

The network-free `rejection_observation_tests` exercise the actual shared async
observer with scripted reads and ready/pending connection futures, including
data before EOF/reset, phase-specific IO errors, separate timeout branches and
all five repeated attempts after failures. These tests establish observer
classification, not OS FIN/RST timing or live Gateway execution. Fresh shared
topology acceptance must separately show real deny-all and qualified-reference
closure, healthy HTTP/bare controls while each negative target is unavailable,
and failures for data/idle targets. Runtime results and platform limits belong
in the scoped implementation record; this page does not infer an executed pass.
Re-evaluate when the probe owner, case registration, deadlines, selected sockets,
supported platform close behavior or Stage1 boundary changes.

## Resolution references

- `ITEST-005` — 2026-08-26.
- `edgionplugins-resource-sweep-config-list-drift` — configuration failure propagation.
- `cache-residue-store-stats-envelope-fail-open` — strict Controller/Gateway
  envelope validation and malformed-envelope regression coverage.
- `ARCH-348` — complete response observations before plugin absence/path assertions.
- `ARCH-338` — native gRPC kind-denial outcomes require the fixture's completed
  no-route envelope instead of generic connection or RPC failure.
- `ARCH-283` — negative TLS probes separate preparation/operation failure from evidenced policy denial.
- `ARCH-341` — raw-TCP policy probes require establishment and prompt accepted-connection closure.
