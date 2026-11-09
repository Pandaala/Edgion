---
name: ldap-auth-search-mode-identity-and-filter
description: Use when reviewing LDAP Auth search mode (bindDn, bindPasswordRef, searchFilter, searchBase) or group requirements (requiredGroupsAnyOf/AllOf, groupAttribute, nestedGroups, 403), LDAP filter injection, username enumeration or timing in search-then-bind, the forwarded identity header, service-account lockout, the TLS requirement for the service password, or control characters in usernames.
status: fixed
closed: 2026-11-09
knowledge_type: regression-guards
---

# LDAP Auth Search Mode — Identity, Filter and Service-Account Guards

## Conclusion and scope

`ldap_auth` search mode (`bindDn` set) runs, per request and on one connection
under the phase-2 deadline: service bind → whole-subtree search (sizeLimit 2,
requesting only `attribute`) → user bind with the found entry's DN → unbind. The
following invariants hold and must keep holding:

1. **No filter injection.** `searchFilter` is compiled once
   (`edgion_ldap::filter::SearchFilter`) into a flat encoding-op stream; the
   username is inserted as raw assertion-value bytes and never parsed as filter
   text. `{username}` is accepted only inside assertion values; at most 8 slots,
   4096 filter bytes, nesting depth 16; search-mode usernames are capped at 1024
   bytes. Do not "fix" this by RFC 4515-escaping a username into a string filter.
2. **Uniform 401 for every per-user outcome.** No entry, more than one entry,
   sizeLimitExceeded, an entry without exactly one valid `attribute` value
   (non-empty, no leading/trailing space, no control characters), an entry whose
   DN is empty or unparsable, and the service account's own entry all map to
   `InvalidCredentials` with the
   `Deny bad credential` tag and the failure delay — identical to a wrong
   password. `BindSession::search_unique` returns `Option<FoundEntry>`, so the
   cases cannot be told apart by callers.
3. **Canonical identity.** The identity header carries the entry's single
   `attribute` value, never the client-typed string (which may be an alias under
   `|`, substrings or `~=` filters).
4. **Never bind as the service account for a client.** A found DN equivalent to
   `bindDn` (`edgion_ldap::dn::equivalent`) is rejected before the user bind:
   otherwise clients could log in as it or lock it out. The comparison
   approximates the directory's `distinguishedNameMatch`: canonical names for
   the common naming attributes and their OIDs (`cn`/`commonName`/`2.5.4.3`,
   `uid`, `dc`, `ou`, `o`, ...), decoded escapes and `#hex` BER strings,
   Unicode lowercase values, RFC 4518 space handling, AVA order and `;`
   separators. `bindDn` must be in plain string form (`dn::is_string_form`:
   descriptor types, no `#hex`), so the operator side is always canonical, and
   it is normalized once per plugin generation (`SearchPlan::service_dn`); an
   unparsable found DN is denied rather than compared. The effective search
   base must also parse as a DN at admission. Docs require the entry DN,
   not a UPN.
5. **Service password.** `bindPasswordRef` (Secret key `password`) resolves like
   OIDC `clientSecretRef`; `resolvedBindPassword` is `redact_opt` +
   `#[schemars(skip)]`. Search mode requires `ldaps` or `startTls` with
   certificate verification (a local `tls.verify: false` without SAN policy is
   rejected; an inherited global `verify=false` logs a build-time WARN).
6. **Failure statuses.** A rejected or unresolved service password → generic 401
   (`skills/00-design-rules.md` generic-401 rule) plus metric
   `service_bind_fail`; other service-bind results, search errors, referral
   results (rc 10) and > 8 references → 503 `ldap search fail` / `search_fail`.
7. **Identity-header hardening (all modes).** A trimmed username containing a
   control character (Unicode `Cc`, including TAB) is rejected before any LDAP
   I/O; a failed identity or anonymous header write denies the request
   (`identity header rejected`); header names and the `anonymous` value are
   validated at admission; every input-only 401 applies the failure delay.
   `LdapAuth::new` runs `get_validation_error` itself, so search mode can never
   be built over plaintext even outside the runtime gate. Before 2026-11-09 a
   failed write was ignored and a client-supplied `X-Credential-Identifier`
   reached the upstream.

8. **Group requirements run only after the password is verified.** With
   `requiredGroupsAnyOf`/`requiredGroupsAllOf`, the order is service bind →
   search → user bind → (rc 0 only) service re-bind → base-scope
   `search_unique(found DN, Scope::Base, group filter, "1.1")` → unbind. A
   wrong password or unknown user never reaches the group step, so 403, a
   failing or slow group search, and group-dependent timing are visible only
   with the correct password; the unknown-user timing residual stays one round
   trip. The group filter (`SearchFilter::group_requirement`, `&`(all-of,
   `|`(any-of))) is built and pre-encoded once per generation; group DNs are raw
   assertion bytes. Match → success; no entry (also rc 4 or a second entry) →
   403 `Deny not in required group` + delay, no identity header; re-bind rc 49
   → generic 401 `service_bind_fail`; other re-bind/search failures → 503
   `search_fail`. An explicit `[]` or `null` list is rejected by validation
   (`null` decodes as an empty list on purpose: a decode error would drop the
   whole EdgionPlugins object and break the Kubernetes list).
   Moving the group step before the user bind was rejected at review: it gave
   existing users a password-free 503 (existence oracle) and membership timing.

## Rationale and evidence

- Design and review record: local spec
  `docs/superpowers/specs/2026-11-09-ldap-auth-search-mode-design.md` (48 P3
  findings dispositioned before implementation).
- Code: `edgion-ldap/src/filter.rs`, `dn.rs`, `proto.rs` (`encode_search_request`,
  `decode_search_frame`), `client.rs` (`search_unique`);
  `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` (`search_phase`,
  `search_steps`, `run_request`); validation in
  `edgion-resources/.../plugin_configs/ldap_auth.rs`.
- `edgion-resources` depends on `edgion-ldap` with default features off: the
  `client` feature (tokio) stays out of the schema crate's declared edges.

## Alternatives and limits

- **Timing residual (accepted, not masked).** An unknown user skips the final
  bind and answers one LDAP round trip faster than a wrong password.
  `authFailureDelayMs` slows guessing but does not equalize timing
  ([basic-auth username enumeration](../../auth-timing/regression-guards/basic-auth-username-enumeration.md));
  `RateLimitLocal` bounds probing. A dummy bind was rejected: it doubles
  directory load and directories may still answer non-existent DNs at a
  different speed.
- **Uniqueness across partitions.** An entry plus continuation references with
  Done rc 0 is accepted: Active Directory returns DomainDnsZones/ForestDnsZones
  references on ordinary subtree searches. A duplicate in a referred partition is
  not detected (referral chasing is a non-goal).
- **Client-triggerable 503.** A directory that rejects an assertion value for
  some usernames (rc 18/21/87) answers 503, the same class as bind mode.
- **Schema aliases** are not resolved for `attribute`; a multi-valued
  `attribute` denies that user.
- **DN comparison residual.** `dn::equivalent` does not apply full Unicode
  normalization (NFKC, RFC 4518 mapping tables) and knows only the alias table
  in `dn.rs`. A directory returning the service DN with another schema alias or
  a non-NFKC-equal spelling would escape the guard; directories return their own
  stored spelling, which matches an operator copy of it. The robust alternative,
  an RFC 4532 Who Am I? after the service bind, costs one more round trip per
  request and was rejected for that reason.
- **`groupAttribute` must not be user-writable.** Membership is read from the
  user's own entry. OpenLDAP 2.4's memberof overlay attribute is not
  NO-USER-MODIFICATION and is not reconciled (`olcMemberOfDangling: ignore`),
  so with a `by self write` ACL a user can grant themselves a group (reproduced
  on the integration image at review). The gateway cannot detect this; docs make
  write protection a deployment prerequisite. AD `memberOf` is a system
  back-link. Do not report "user can set memberOf" against the plugin unless
  the docs lose the prerequisite.
- **401 vs 403 confirms a non-member's password.** Accepted product decision
  (Kong parity); docs point to putting the group in `searchFilter` instead.
- **Bind mode forwards the client's spelling.** In bind mode the identity
  header carries the trimmed username as typed (`ALICE` for `uid=alice`), since
  no entry is read; search mode forwards the canonical value. Tracked in
  `tasks/todo/ldap-auth-follow-ups.md`.
- **No pool.** Three round trips (five with group requirements on a
  successful login) on one per-request connection
  ([per-request connection](../intentional-design/per-request-connection-accepted.md)).
- **Interop.** OpenLDAP verified; no Active Directory run yet
  (`tasks/todo/ldap-auth-follow-ups.md`).

## Verification and re-evaluation

Fresh on 2026-11-09:

- `cargo test -p edgion-ldap`: 66 passed (filter BER per kind, injection
  vectors, limits at and past each cap, DN equivalence across directory
  spellings, search codec, `search_unique` at the reference cap, group filter BER and pre-encoding, base-scope requests); 40 passed with
  `--no-default-features`.
- `cargo test -p edgion-gateway --lib ldap_auth`: 65 passed, including search
  mapping, canonical identity, service-DN guard over alias/OID/hex spellings
  (mutation-checked), injection bytes on the wire, pre-I/O rejections proven
  against an in-memory peer that receives zero bytes, the 1024-byte boundary,
  metric increments, the failure delay on input-only 401s (paused time),
  header hardening, constructor validation.
- `cargo test -p edgion-resources` (validation table), `-p edgion-controller`
  (password resolution, forged-value clearing, read inventory).
- cargo-fuzz `ldap_decode` with search frames and filter compile: 14,457,688
  executions; with DN parse/equivalence added: 2,610,629 executions; no crash.
- Phase 2b (groups): `edgion-ldap` group filter BER, base-scope bytes and
  match tests; gateway wire sequences (member: six requests; wrong password:
  four, no group step), outcome mapping incl. second entry, re-bind rc 49 /
  protocol error, group-frame protocol error, re-bind and group timeouts, timeout, 403 body/tag/delay with no
  identity write; resources validation incl. `null` decoding at the
  EdgionPlugins level. Integration: member any-of 200, non-member 403,
  non-member wrong password 401, alice (gw-users) any-of 200, all-of member
  200, all-of missing one 403 (LdapAuthTls 16/16).
- Integration (real OpenLDAP): LdapAuthTls (five search cases: success whose
  forwarded `x-credential-identifier` must equal `carol` exactly while the
  client typed `CAROL` and sent a spoofed header, wrong password, unknown,
  ambiguous, wrong service password → 401); LdapAuth; egress.

Re-evaluate when: the group step moves before the user bind or gains another
search (Phase 2b fired the original trigger and is recorded in invariant 8); a
connection pool or result cache is introduced; the filter limits change; a
dummy bind or other timing equalization is proposed; any new 401-path tag depends
on the searched user.

## Resolution references

- Phase 2a search mode (`tasks/todo/ldap-auth-search-mode.md`), 2026-11-09.
- Phase 2b group requirements, 2026-11-09: design and review record in local
  spec `docs/superpowers/specs/2026-11-09-ldap-auth-group-requirements-design.md`.
- Pre-existing identity-header defect found at P3 review, fixed 2026-11-09.
