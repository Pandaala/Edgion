---
name: ldap-auth-dial-time-egress-policy
description: Use when reviewing LDAP Auth SSRF or DNS findings that ldap3 resolves ldapHost without EgressResolver, or when a fix proposes URL rewriting, controller pre-resolution, or a plugin-local address HashMap.
status: fixed
finding_id: D0053
closed: 2026-09-04
knowledge_type: regression-guards
---

# LDAP Auth Dial-time Egress Policy — FIXED

## Conclusion

`ldap_auth` must classify every LDAP destination at connect time through the
shared Gateway `EgressResolver` and connect only to admitted addresses, while
keeping the original `ldapHost` URL for LDAPS and StartTLS identity.

Fix suggestions of the form "rewrite the LDAP URL to a resolved IP", "preflight
DNS in the Controller or at plugin construction", "store addresses in LinkSys
or a new plugin HashMap", and "let ldap3 resolve the hostname itself after a
config-time check" are **not accepted**.

## Core Rationale

**1. Dial-time classification closes DNS rebinding**

`ldapHost` is an operator-configured hostname, not a Kubernetes Service
backendRef. ldap3's default `TcpStream::connect(host:port)` uses the system
resolver and never shows Edgion the candidate IPs. `EgressResolver` classifies
the exact addresses about to be dialed, matching webhook URL targets, Wasm
callouts, and ext_proc.

**2. The original hostname remains the TLS identity**

ldap3 `LdapConnSettings::set_std_stream` accepts an already-connected TCP
stream. The plugin still passes `ldap://host:port` or `ldaps://host:port`, so
SNI, certificate verification, custom CA, SAN pinning, and mTLS keep using
`ldapHost`.

**3. DNS answers are cached by the canonical egress answer cache**

`LdapAuth` holds an `EgressResolver`, which resolves through the process-wide
egress `DnsAnswerCache` (TTL-honoring, same-key misses coalesced, raw answers
classified on every use; common-02). A second `HashMap`/`ArcSwap` on the
plugin, in LinkSys, or on the CR is a duplicate cache. See
[DNS answer cache identity and lifecycle](../../backends/regression-guards/dns-answer-cache-identity-and-lifecycle.md).

**4. Multi-address fallback matches ext_proc**

Refused candidates try the next admitted address. A SYN-blackholed first
address consumes the plugin timeout, so later addresses may not run. Per-attempt
connect timeouts are not accepted here; they would diverge from
`ext_proc::connect_admitted` without a shared dial helper.

## Cold TLS ownership and activation

For LDAPS and StartTLS, Gateway compilation checks the actual LDAP plugin entry,
its declared local TLS references and carried Secret/ConfigMap provenance before
entry-cache reuse or certificate parsing. Common owns the canonical material and
inheritance checks. One captured global-default snapshot is used throughout
preflight and TLS construction; construction never reloads ambient defaults.

The actual source owner is checked for every transport. Plain LDAP consumes no
local TLS material or global TLS policy, so inactive invalid TLS references,
carriers or defaults cannot change plaintext construction. Disabled entries stay
inert. Omitted local owner/reference namespaces retain canonical default behavior;
carried plugin material must have explicit matching namespace metadata.

EntryCache belongs to one immutable plugin generation. Same-generation reuse is
valid; reference/material replacement builds a fresh generation/cache. Negative
candidates tested against an occupied cache must fail preflight, not reuse success.
The existing dial-time hostname, stream injection, SAN/M4, timeout and authentication
contracts remain unchanged. Focused construction/effective-material tests do not
claim a new live LDAP handshake or Gateway API conformance run. Tests mutating global
defaults use the existing exact-child helper with normal restoration and process
teardown on panic; Common's test-only RAII guard is not widened across crates.

## Fix Suggestions Not Accepted

- Rewriting the LDAP URL to an IP — breaks LDAPS/StartTLS hostname verification
- Controller or plugin-init DNS with a later hostname connect — TOCTOU / DNS rebinding
- A new LinkSys or plugin-local address table — duplicates `EgressResolver`
- Weakening existing LDAP integration assertions — D0054 adds independent
  cross-process coverage instead.

## Cross-process regression coverage

D0054 (`add-ldap-auth-egress-integration-coverage`) adds
`edgion-tests/integration/scripts/integration/test_ldap_auth_egress.sh` and its
`ldap_egress/` fixtures. The standalone exception is process-local system
resolution: the Gateway must start with a loader fixture because `ldap_auth`
uses `ResolverSpec::System`. Host DNS and production resolver code are not
modified. All scenario generations share one Controller/Gateway pair, and
updates are applied through the Controller Admin API.

The fixture supplies an ordered denied IPv6 unspecified address followed by
allowed IPv4 loopback (per the existing integration-mode exception). Both
unspecified address families are used for the all-blocked case. A calibrated,
reachable IPv6 trap and an opaque IPv4 TCP relay provide connection counts;
the canonical egress metric proves that candidates were actually classified.
The `osixia/openldap:1.5.0` fixture performs the real handshake and bind behind the relay, which
observes the original ClientHello SNI without terminating TLS.

`LdapAuthConfig.ldap_host` remains the identity used by `LdapAuth.ldap_url`,
while `egress` selects the fixed socket and `conn_settings` carries the TLS
policy. Both LDAPS and StartTLS use a DNS-only certificate and exercise wrong
hostname, wrong custom CA, SAN match/mismatch, and required mTLS with/without
the resolved client identity. All-blocked results must be the fixed public
503 body with the `ldap unreachable` PluginLog tag and `unreachable` error
metric. A subsequent allowed generation must recover without process restart.
The complete matrix repeats twice per invocation.

This coverage does not prove every OS resolver's DNS behavior, private-network
policy, arbitrary LDAP-server interoperability, or timeout fairness. The
system resolver replacement is limited to three reserved fixture names and
does not replace TCP, the egress filter, ldap3, TLS, or OpenLDAP. Existing
LDAP integration scenarios remain unchanged. User configuration, defaults,
and production behavior are unchanged.

### Shared LDAP TLS fixture prerequisite (ARCH-373)

The Kubernetes shared suite previously mapped both LDAPS ports to the same
`LDAP_TLS_VERIFY_CLIENT=try` server. Its identity-present 200 assertion did not
establish that client identity was required. The separate
`edgion-test-openldap-mtls` Deployment now owns `demand` on Service port 33637;
CA-only LDAPS and STARTTLS retain the optional-client-certificate server.

Both local and Kubernetes fixture trees expose a matching identity-absent route.
The shared `mtls_without_client_cert_returns_503` case calibrates identity-present
200, identity-absent 503 with the fixed LDAP failure body, then identity-present
200 against the same required endpoint and otherwise identical plugin settings.
All four original case identities and assertions remain. TCP/process readiness
alone does not prove certificate enforcement; the calibrated bind matrix does.

The existing LDAP certificate generator is shared by both preparation paths.
Kubernetes uses a run-owned output directory and generates matching CA, client,
server and wrong-CA Secrets. Server SANs cover both Service FQDNs; cached local
certificates missing those identities are regenerated. The four empty Kubernetes
Secret templates are excluded by both apply entrypoints and the ownership
validator, so static apply cannot replace prepared material. Preparation restarts
both LDAP Deployments because their init containers copy certificate files;
reused readiness requires both Deployments and the required Service port.
Namespace-owned cleanup remains unchanged.

Fresh ARCH-373 verification on 2026-10-15 passed all five local shared-suite
cases through the ordinary selected runner, including the calibrated control,
wrong CA and STARTTLS. Certificate cache/SAN regeneration and both apply-entrypoint
exclusions passed isolated checks. Workspace check, Clippy and formatting passed.
The selected local runner exempts only the exact LDAP fixture list from unrelated
macOS LB/GeoIP loopback-alias prerequisites; broader selections retain that gate.
An isolated, owned Kubernetes namespace also passed real optional LDAPS and
STARTTLS binds plus required-endpoint identity-present/absent/present probes.
Those optional-endpoint probes retained container LDAP client-identity defaults;
they prove successful binds, not identity absence. CA-only/no-client-identity
success is established by the local shared suite.
The absent identity received a TLS EOF; connection failures were not accepted.
The explicit client used Python SSL with no implicit LDAP client identity.
This fixture-only check used translated namespace SANs and Service port-forwarding,
not a Gateway. These results are separate from historical D0054 evidence below.
Full Kubernetes Gateway-to-LDAP acceptance requires a healthy shared
Controller/Gateway installation; fixture-only handshakes cannot substitute for
that end-to-end gate. Revisit this boundary when server client-auth policy,
certificate generation, Secret apply paths or suite routes change.

### Accepted failure-path container retention

On 2026-09-07, the user accepted retaining OpenLDAP containers after a timeout
or interrupted cleanup for inspection. `Scenario.close()` attempts cleanup,
but its Gateway wait can consume the outer deadline runner's default 10-second
termination grace before container removal; Docker commands also have a
60-second per-command timeout. Container removal on these failure paths is
therefore not guaranteed and is not a D0054 acceptance gate. Retained containers
consume Docker resources until manually removed. The integration scripts README
documents identification by container name and certificate bind mount, inspection,
and removal for one run. This decision is specific to the standalone LDAP
scenario; it does not change shared runner lifecycle requirements. Re-evaluate
if unattended repeated runs require automatic reclamation or retained containers
cause resource pressure.

## Verification

Fresh checks on 2026-09-07:

- `cargo build -p edgion-gateway -p edgion-controller -p edgion-cli --bins`
  passed before the final scenario runs.
- `bash edgion-tests/integration/scripts/integration/test_ldap_auth_egress.sh`
  passed the 36-stage matrix repeatedly on macOS arm64, including the
  API-only update path. Each invocation creates fresh certificates and
  OpenLDAP containers and performs two full matrix repetitions.
- A Debian bookworm arm64 container with Bash 5.2 compiled the Linux fixture
  through `--build-resolver-only`; loader probes verified ordered results for
  all three names, ordinary libc fallback, and `freeaddrinfo` ownership.
  Additional probes on macOS and Linux verified byte-fragmented LDAPS and
  StartTLS ClientHello parsing and reachability of the unspecified IPv6 trap.
  This is Linux fixture validation, not a full Linux Gateway matrix run.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed; Clippy emitted existing
  warnings outside this test-only change.
- Agent-doc, SSA-force, metrics-inventory, Pingora-source-policy,
  English-only and Bash arithmetic guards passed. Changed shell scripts
  passed syntax checks with macOS Bash and Linux Bash 5.
- Independent read-only review approved the test boundary and knowledge
  classification. Existing LDAP test files and product Rust code were not
  modified; the full `cargo test --all` suite was not run.

The D0053 closure date above is historical implementation evidence. D0054
adds cross-process verification; it does not reopen or change that runtime
implementation. Revisit the fixture when libc loader/interposition behavior,
the LDAP TLS dependency, egress defaults, or diagnostic vocabulary changes.

## Re-evaluation Triggers

Re-open this decision only if:

- ldap3 loses the pre-connected stream injection API
- LDAP gains a Kubernetes Service `backendRef` target that should follow the
  EndpointSlice path instead of hostname egress
- Gateway egress defaults change so that typical in-cluster LDAP would be
  denied without a plugin-local override

## Reference Cases

- Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`
- Sibling: `skills/04-review/ssrf/regression-guards/forward-auth-webhook-ssrf.md`

## Resolution references

- `D0053` — 2026-09-04.
- `D0054` / `add-ldap-auth-egress-integration-coverage` — verified test gap
  corrected with independent egress/TLS integration coverage, 2026-09-07.
