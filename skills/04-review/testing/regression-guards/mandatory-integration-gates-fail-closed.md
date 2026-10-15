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

## Resolution references

- `ITEST-005` — 2026-08-26.
- `edgionplugins-resource-sweep-config-list-drift` — configuration failure propagation.
- `cache-residue-store-stats-envelope-fail-open` — strict Controller/Gateway
  envelope validation and malformed-envelope regression coverage.
- `ARCH-348` — complete response observations before plugin absence/path assertions.
