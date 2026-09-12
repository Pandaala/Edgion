---
name: credential-copy-retention
description: Use when reviewing proposals to remove, move, or zeroize existing authentication, managed-credential, TLS, or ACME credential owners.
status: accepted-tradeoff
finding_id: D0036
closed: 2026-09-12
knowledge_type: intentional-design
---

# Residual credential copies are retained without a broad ownership rewrite

## Conclusion and scope

The `auth-credential-ownership-followups` review closed without further code
changes on 2026-09-12 after the user explicitly accepted the bounded residual
owners below. This is a value judgment: the copies exist, but their removal has no
demonstrated security or operational benefit proportional to its cost. It is not a
finding that credentials are physically erased, nor a blanket exemption for new or
measurably expensive copies.

The decision covers the current BasicAuth and HMAC Controller parsers, HMAC and JWT
Gateway runtimes, managed CredentialInjector publication/injection, shared outbound
TLS material projection, and ACME EAB account construction. The completed KeyAuth
parser reduction remains a separate correction. DNS-01 has been removed and is not
authorized by this decision.

## Rationale and evidence

The source-backed owner and release-event inventory is maintained in
[credential copy ownership evidence](../review-evidence/credential-copy-ownership.md).
The residual review reached these independent decisions:

| Boundary | Decision | Decisive evidence, cost, and residual risk |
| --- | --- | --- |
| BasicAuth parser seen map | Keep | It duplicates usernames and group names, not passwords. The final map could perform first-wins detection, but the saved cold-path allocation has no demonstrated security or operational impact |
| HMAC parser seen map and selected YAML scalars | Keep | The seen map duplicates identity metadata. Parsed YAML briefly overlaps the final HMAC key and metadata, but the fetched Secret and published generations remain independent longer-lived owners; removing this overlap does not establish erasure |
| HMAC runtime map | Keep | `HmacVerifier` needs an immutable map independent of the shared source configuration. Removing it requires changing construction/ownership without eliminating the source generation |
| JWT per-issuer groups and request token | Keep | Per-issuer groups duplicate credentials in transport, but changing the shape crosses Controller, schema/transport and Gateway. The request token String unifies header/query/cookie ownership; removing only the header copy leaves the session's original token and adds hot-path lifetime complexity |
| Managed credentials and injected headers | Keep | Bootstrap, retained publication, conf-sync material, sensitive runtime HeaderValue and attempt-local header have different retry, publication, and request duties. Credential injection intentionally sends the configured credential upstream |
| Shared TLS material projection | Keep | Full Secret projection can retain unrelated fields, but replacing it requires one typed transport shared by all Controller, Common, and Gateway TLS consumers. Local zeroization cannot erase source stores, retired generations, or TLS-library state |
| ACME EAB construction | Keep | Fetch, Base64 decode, and the ACME SDK own separate cold-path buffers. Removing one temporary copy has negligible exposure reduction while the Kubernetes Secret and SDK account operation remain |

Envoy is comparative evidence, not authority for Edgion. Its
[BasicAuth parser](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/http/basic_auth/config.cc)
uses the result map for duplicate detection, but still constructs separate line,
username, and hash strings and stores the username as both a map key and `User.name`.
Its [request filter](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/http/basic_auth/basic_auth_filter.cc)
borrows username/password views from one decoded buffer but creates digest scratch
without a general zeroization claim. Envoy's static and thread-local Secret provider
[types](https://github.com/envoyproxy/envoy/blob/main/source/common/secret/secret_provider_impl.h)
and [publication implementation](https://github.com/envoyproxy/envoy/blob/main/source/common/secret/secret_provider_impl.cc)
copy protobuf secrets and publish owned strings to worker-local generations. Its
[Secret manager](https://github.com/envoyproxy/envoy/blob/main/source/common/secret/secret_manager_impl.cc)
redacts admin dumps rather than promising immediate destruction of every buffer.

This comparison supports treating bounded owner copies as normal implementation
boundaries. Edgion's own redaction, sensitive-header marking, schema/transport,
fail-closed authentication, hot-reload, retry, and access-log rules remain the
authority.

## Alternatives and limits

The rejected alternative was a broad move/`Zeroizing` migration. It would touch
unrelated ownership models, could interfere with shared generations and retries,
and could overstate the protection because allocator, transport, SDK, source Secret,
and crypto-schedule owners remain.

The accepted cost is bounded extra residency until each recorded owner releases its
generation or request-local value. A Rust drop or validity deadline is still not a
physical-erasure guarantee. This decision does not permit logging credentials,
removing redaction, forwarding authentication material except where the configured
CredentialInjector explicitly requires it, or adding a new owner without need.

## Verification and re-evaluation

This closure changes review knowledge only. It does not change runtime behavior,
configuration, user documentation, authentication results, forwarding, hot reload,
or integration fixtures, so no runtime test is required for the disposition itself.
Fresh closure verification on 2026-09-12 passed `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
the agent-document consistency guard, the SSA-force guard, the metrics-inventory
guard, the Pingora-source-policy guard, and `git diff --check`. Clippy reported
existing workspace warnings, including `manual_contains` in
`edgion-tests/src/client/test_client.rs`; none are introduced by this
knowledge-only diff. An independent read-only review approved the source facts,
classification, links, and no-code disposition after the classification was
corrected to intentional design. No full `cargo test --all` or end-to-end topology
was run because runtime code and behavior are unchanged.

Re-evaluate an individual boundary when:

- profiling shows material resolver, configuration-generation, or request hot-path
  memory pressure;
- credential documents or multi-issuer JWT groups become large in supported use;
- plugin construction can consume a uniquely owned configuration;
- retry, publication, transport, or TLS consumer contracts change;
- an SDK accepts ownership of a zeroizing buffer without another copy;
- concrete leak or exposure evidence appears; or
- an otherwise-required rewrite makes the copy removable at negligible risk.

Do not reopen every boundary when only one trigger fires. Reassess the affected
owner, its remaining independent copies, and the actual exposure reduction.

## Resolution references

- `auth-credential-ownership-followups` / D0036 follow-up — accepted no-code
  closure on 2026-09-12 after user review of value and Envoy comparison.
- [Credential owner and release-event inventory](../review-evidence/credential-copy-ownership.md).
