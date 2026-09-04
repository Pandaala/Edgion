---
name: ldap-auth-dial-time-egress-policy
description: Use when reviewing LDAP Auth SSRF or DNS findings that ldap3 resolves ldapHost without EgressResolver, or when a fix proposes URL rewriting, controller pre-resolution, or a plugin-local address HashMap.
status: fixed
finding_id: D0053
closed: 2026-09-04
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

**3. The address memo already lives on the plugin instance**

`LdapAuth` holds an `EgressResolver`. That resolver's bounded TTL HashMap is
the canonical per-instance memo. A second `HashMap`/`ArcSwap` on the plugin,
in LinkSys, or on the CR is a duplicate cache. Concurrent first misses may
each resolve DNS; that stampede is accepted.

**4. Multi-address fallback matches ext_proc**

Refused candidates try the next admitted address. A SYN-blackholed first
address consumes the plugin timeout, so later addresses may not run. Per-attempt
connect timeouts are not accepted here; they would diverge from
`ext_proc::connect_admitted` without a shared dial helper.

## Fix Suggestions Not Accepted

- Rewriting the LDAP URL to an IP — breaks LDAPS/StartTLS hostname verification
- Controller or plugin-init DNS with a later hostname connect — TOCTOU / DNS rebinding
- A new LinkSys or plugin-local address table — duplicates `EgressResolver`
- Changing existing LDAP integration tests — tracked separately as D0054

## Re-evaluation Triggers

Re-open this decision only if:

- ldap3 loses the pre-connected stream injection API
- LDAP gains a Kubernetes Service `backendRef` target that should follow the
  EndpointSlice path instead of hostname egress
- Gateway egress defaults change so that typical in-cluster LDAP would be
  denied without a plugin-local override

## Reference Cases

- D0053 — `tasks/pending/edgion-gateway-file-optimization/deferred/D0053-ldap-auth-dial-time-egress-policy.md`
- Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`
- Sibling: `skills/04-review/ssrf/forward-auth-webhook-ssrf.md`
