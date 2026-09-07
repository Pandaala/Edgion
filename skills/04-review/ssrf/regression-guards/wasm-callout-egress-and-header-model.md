---
name: wasm-callout-egress-and-header-model
description: Use when reviewing SSRF / header-hygiene findings on the Wasm plugin's proxy_dispatch_http_call callout path (edgion-gateway/src/plugins/http/wasm/callout.rs, host_abi.rs). Covers what IS guarded (DNS-rebinding via EgressResolver, redirect-Deny, allowlist), and the accepted tradeoffs (IP-literal-origin operator self-harm, guest-controlled callout headers, no aggregate wall-clock budget).
knowledge_type: regression-guards
---

# Wasm callout SSRF + header model — GUARDED + accepted tradeoffs

## What the callout path guards (do NOT re-flag as missing)

`execute_callout` (`callout.rs`) uses a dedicated `callout_client()` built via
`build_outbound_client` with:

- **`EgressResolver`** (`dns_resolver`) → classifies the exact IPs reqwest is
  about to connect to, closing the DNS-rebinding / TOCTOU window on an
  allowlisted **hostname** origin (loopback, link-local incl. 169.254.169.254,
  and — when `outbound_egress.block_private` is on — RFC1918/ULA are dropped).
  This is the same guard the webhook / forward_auth path uses.
- **`OutboundRedirect::Deny`** → no redirect-based escape, and no
  same-host `https`→`http` downgrade that would forward a header.
- **`calloutAllowlist`** exact-origin match. The single normalizer
  `normalize_callout_origin` (edgion-resources `wasm.rs`) is used at admission,
  plugin build, and dispatch, so admission and runtime cannot disagree. The
  connection target is built from the **normalized origin**, never raw guest
  bytes; `:path` is forced to begin with `/`, so authority injection via `:path`
  is impossible. Empty allowlist = deny all callouts.

The bytecode-fetch path (`vm_manager.rs::fetch_url_bytes`) uses the same
egress-guarded + redirect-Deny client, and `sha256` is still enforced on the
final bytes.

## Accepted tradeoffs (fix suggestions here should be CLOSED unless a new scenario is shown)

**1. IP-literal origins are not additionally IP-classified at dispatch.**
reqwest skips the custom DNS resolver for IP literals, so an operator who
allowlists `http://169.254.169.254:80` or an RFC1918 literal reaches it. This is
**operator self-harm**: the operator explicitly put that exact IP in
`calloutAllowlist`. The DNS-rebinding threat (an allowlisted *hostname*
resolving to an internal IP) — the real untrusted-guest vector — IS closed by
`EgressResolver`. Replicating the webhook `dial_url` literal guard for callouts
was judged disproportionate to operator-self-harm-only residual.

**2. Guest fully controls callout request headers (no hop-by-hop filtering).**
By proxy-wasm design the guest builds the callout request; non-pseudo headers
are forwarded verbatim (CRLF injection is still blocked — `HeaderValue::from_str`
rejects it). No `is_hop_by_hop` stripping is applied. Rationale: the guest is the
plugin author's own code and the target is operator-allowlisted, so header
control is intended, not untrusted. A guest-set `Host` does not move the
connection (reqwest connects by URL), so it is not an allowlist bypass.

**3. No aggregate paused-request wall-clock budget.**
A guest can add up to `MAX_CALLOUTS_PER_REQUEST` (8) × `calloutTimeoutMs` of
latency across sequential callouts. Bounded and operator-configured
(`calloutTimeoutMs` default 5s); a single "total callout wall-clock" ceiling was
not added. Per-callout timeout + the 8-dispatch / 4-round caps + the global
64-concurrent-callout semaphore bound the blast radius.

## Related

- Fixed this session (not tradeoffs — real bugs corrected): the fetch no-TLS
  branch previously followed redirects with no egress guard; the callout used the
  plain shared client with no `EgressResolver`. Both now route through the
  hardened builder. `read_guest` now bounds-checks `len` against guest memory
  before allocating (was an unbounded pre-alloc). See [[dsl-http-call-ip-literal-only]]
  for the sibling DSL decision on IP-literal-only guards.
