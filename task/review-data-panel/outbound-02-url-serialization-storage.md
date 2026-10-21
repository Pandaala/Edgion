# Outbound URL serialization ownership

Status: deferred for user decision. No runtime change.
Source baseline: c41e16633.

## Evidence and scope

`replace_url_host` in
`edgion-gateway/src/plugins/http/common/outbound/call.rs` parses a base URL,
replaces its host, then copies the resulting serialization with `to_string()`.
The successful branch could consume the URL with `String::from(url)` instead.
Locked `url` 2.5.8 implements that conversion by moving `Url::serialization`;
its Display implementation writes the same serialization. The error fallback
would still copy the original base. This removes one successful serialization
copy, not URL parsing, host parsing or all intermediate allocations.

The dependency's `set_host_internal` copies the old suffix, truncates the
serialization to the host start, writes the new host, and restores the suffix.
It reuses or grows the existing String. Moving that String therefore preserves
its spare capacity; copying only the final text can produce smaller storage.
A local probe linked against the existing locked dependency artifact compared
both conversions after the same parse/set_host operations:

| Base | Replacement host | Output length | Copied capacity | Moved capacity |
| --- | --- | --- | --- | --- |
| `https://[2001:db8:abcd:1234:5678:9abc:def0:1234]:8443/` | `a.test` | 20 | 20 | 54 |
| `http://10.0.0.1:8080/` | `tenant-long.service.example.test` | 45 | 45 | 84 |

Both rows asserted identical output bytes. These are observed String capacities
on the local build, not stable allocator guarantees, physical heap measurements,
or throughput results. Both shrinking and growing a host can leave spare capacity.

The request caller selects a fresh Service endpoint and effective authority policy,
then stores the helper result in a local Cow. `CompiledRequest::build_request_in`
formats that base with the selected path and parses a separate final request URL.
The local base has no explicit early drop before the retry loop. This source
ownership is not proof of physical retention across every await after optimization.
`client.rs::resolve_probe_base` also uses the helper for the Service policy's
active-health host. Its result feeds separate health-request URL composition.

Only `ResolvedTarget::Service` exposes an authority service key. Service clients
already install AnchorPinnedResolver independently of current authority-policy
presence; the authority changes Host/SNI while destinations stay pinned to ready
endpoints. Static URL targets retain their existing egress behavior. This candidate
requires no resolver, policy, validation, failure, metric or cache changes.

## Decision requested

Retain the current copy, authorize moving the URL serialization with its capacity
tradeoff, or investigate a conversion/lifetime design that avoids the copy without
increasing retained storage. Do not infer a net memory improvement from one fewer
copy. No dependency upgrade or alternate URL assembly algorithm is proposed.

## Acceptance before implementation

Use IMPL with a frozen allow-list for the helper, its actual callers and focused
tests. Preserve exact URL serialization, scheme/port/path/query/fragment behavior,
parse/set_host error fallback, policy lookup timing, authority rejection, pinned
resolution and egress enforcement. Do not replace concatenation with URL joining.
Cover IPv4/IPv6 bases, shorter and longer DNS authority names, default and explicit
ports, invalid-input fallback, request assembly and active-health composition.

Measure allocation calls, requested bytes and retained capacity separately; assess
owner lifetime across request construction, retries and cancellation before making
heap or throughput claims. Existing shared outbound tests provide regression
coverage but are not a live DNS/TLS handshake or performance benchmark. Run the
applicable repository checks and obtain independent final review.

## OIDC redirect outputs (audit at b89c2a072)

The same representation decision applies to two completed URLs in
`plugins/http/openid_connect/`: `callback.rs::build_authorization_redirect`
returns `auth_url.to_string()`, and `logout.rs::build_logout_redirect_target`
returns `end_session_url.to_string()`. A consuming conversion would preserve the
same serialization while retaining its backing capacity. These paths append
query pairs; unlike the earlier outbound examples they do not replace the host.

A fresh standalone probe linked against the existing url 2.5.8 build artifact
asserted byte equality between Display copying and consuming conversion:

| Synthetic case | Output length | Copied capacity | Moved capacity |
| --- | --- | --- | --- |
| Authorization with nonce and S256 fields | 311 | 311 | 528 |
| Logout with redirect and token hint | 123 | 123 | 240 |

Reproduction inputs, in append order, were:

- Parse `https://idp.example.com/authorize`; append `response_type=code`,
  `client_id=sample-client`, `redirect_uri=https://app.example/callback`,
  `scope=openid profile`, `state` with 32 ASCII `a` characters, `nonce` with
  32 ASCII `b` characters, `code_challenge_method=S256`, and `code_challenge`
  with 43 ASCII `c` characters. These fixed values exercise serialization;
  they are not generated authentication credentials or a PKCE verification test.
- Parse `https://idp.example.com/logout`; append
  `post_logout_redirect_uri=https://app.example/after-logout` and
  `id_token_hint=sample-token`.
- For each Url, compute `copied = value.to_string()`, then
  `moved = String::from(value)`; assert equality and read each String capacity.

These are local capacity observations for exact synthetic inputs, not stable
allocator behavior, physical heap accounting, production workload or throughput
results. The probe does not establish which conversion is better overall.

`request_filter_impl.rs` borrows both result Strings into the async
`send_redirect_with_cookies` helper. That helper inserts Location into a separate
response header before awaiting `write_response_header`. This is source-level
ownership evidence, not proof of physical retention across every suspension or
compiler optimization. Any selected lifetime change must assess both owners.

Retain the existing decision alternatives rather than automatically applying
Into<String>. OIDC acceptance additionally covers exact query order/encoding,
existing endpoint query/fragment behavior, nonce/PKCE and state-cookie binding,
reserved authorization parameters, cookie clearing, token hints and invalid/
missing-discovery fallback. Preserve the canonical trusted host and connection-
scheme rules; no forwarded-header trust or authentication policy changes belong
in this optimization.

Fresh formatting, workspace check/Clippy and agent-doc validation passed for this
docs-only extension; existing unrelated warnings remain. No Rust tests, IdP
exchange or performance benchmark ran. The local probe only compared bytes and
capacities; existing OIDC test coverage is not fresh verification of a new API.
