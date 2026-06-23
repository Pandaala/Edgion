---
name: discriminated-error-log-tags
description: Use when reviewing ldap_auth "Deny bad password" / discriminated access-log-tag username-enumeration findings; explains why the enumeration premise is a false positive (LDAP rc 49 is uniform) and records the uniform "Deny bad credential" tag alignment.
---

# LDAP Auth Discriminated Error Log Tags — NOT-AN-ISSUE (security) + convention alignment (fixed F-006, 2026-05-26)

## Conclusion

A finding that `ldap_auth`'s `"Deny bad password"` access-log tag enables
username enumeration ("filter the log for `Deny bad password` to confirm a
username exists in LDAP") is a **false positive on its security premise**.
`ldap_auth` uses LDAP simple-bind: it builds the bind DN from the username
template and attempts a bind. LDAP servers return result code **49
(invalidCredentials) for both a nonexistent DN and a wrong password**
(RFC 4511; OpenLDAP / Active Directory standard behaviour), so the
`rc == 49 → InvalidCredentials → tag` path produces an **identical** tag
whether or not the username exists. The tag therefore cannot — and after
F-006 does not appear to — discriminate username existence.

What F-006 legitimately fixed is **naming consistency**: the tag was renamed
`"Deny bad password"` → `"Deny bad credential"` (plugin.rs InvalidCredentials
arm) to match the uniform failure-tag convention shared by
basic / key / hmac / jwt_auth (see the `group-in-ok-log.md` family).

Fix suggestions of the form "the discriminated `Deny bad password` tag is a
username-enumeration oracle, downgrade/redact it for that security reason" are
**not accepted** — the oracle does not exist.

## Core Rationale

**1. LDAP rc 49 is uniform across nonexistent-user and wrong-password.**

`ldap_bind` (`edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`) calls
`simple_bind(bind_dn, password)` and maps `rc == 49` →
`LdapAuthError::InvalidCredentials`. There is no preceding LDAP *search* to
test existence; a bind against a DN that does not exist returns 49 exactly as
a bind with a wrong password does. A log reader sees the same tag in both
cases, so no existence bit leaks.

**2. The only username-discriminating tag fires on empty input only.**

`"Deny bad username"` (the `build_bind_dn` Err arm) triggers solely on an
**empty username** (the only failure branch of `build_bind_dn`; `ldap3::dn_escape`
and template substitution do not fail). An empty username is trivially not a
real account, so this tag carries zero enumeration value either.

**3. HTTP status already separates credential vs infrastructure failures.**

`InvalidCredentials` → 401; `ConnectionFailed` / `Timeout` / `BindFailed` →
503. Any credential-vs-infra signal in the log tags is already disclosed by
the response status, so the tags add no discrimination beyond the response.

**4. The rename is a convention-consistency change, not a security fix.**

`basic_auth` deliberately uses the uniform `"Deny bad credential"` literal and
the `group-in-ok-log.md` rule explicitly rejects splitting it into
`Deny unknown user` vs `Deny wrong password`. Aligning `ldap_auth` to the same
literal removes the misleading "password specifically was wrong" implication
and keeps the auth-plugin family uniform.

## Fix Suggestions Not Accepted

- "Redact / coarsen the tag because it is an enumeration oracle" — the oracle
  premise is false (rc 49 is uniform); the rename was made for naming
  consistency, not to close a leak.
- "Re-rate this Medium/High because logs reveal username existence" — they do
  not; reject re-reports at any severity above the closed convention nit.
- "Also fold `Deny bad username` into `Deny bad credential`" — out of scope;
  it fires only on empty username (no existence value) and names a distinct,
  legitimate malformed-input case.

## Re-evaluation Triggers

Re-open only if:

- `ldap_auth` is changed to a **search-then-bind** flow that performs an LDAP
  search to locate the DN before binding — a "no such object" search result
  *would* discriminate existence and a distinct tag on that path would leak.
- A new arm is added that emits a username- or existence-dependent tag on the
  401 path.

## Reference Cases

- F-006 (2026-05-26) — `tasks/plugins/plugins-http-F006-ldap-auth-discriminated-error-log-tags.md`
- Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` (`ldap_bind` rc-49 mapping; InvalidCredentials log arm).
- Convention family: `skills/04-review/basic-auth/group-in-ok-log.md` (uniform `Deny bad credential`; rejects unknown-user/wrong-password split), and the matching `key-auth` / `hmac-auth` / `jwt-auth` `group-in-ok-log.md`.
- Sibling ldap decision: `skills/04-review/auth-timing/ldap-auth-cache-timing-oracle.md`.
