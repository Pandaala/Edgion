# Structured L4 peer-address handoff

Status: deferred construction and normalization decision for the user.
Source baseline: 5b4b6a9f3. No runtime or dependency change.

## Evidence

TCP handle_connection in routes/tcp/edgion_tcp.rs formats the selected Pingora
socket address into TcpContext.upstream_addr, validates the original enum, then
passes the text to BasicPeer::new. The String remains required by connection
logging under the current context contract.

TLS termination and passthrough in routes/tls/{proxy,passthrough_proxy}.rs each
format the selected address and parse it for the structured log's IP and port
before safety validation. BasicPeer::new parses the same text again after the
guard. A successful connection carries the String to the shared
send_proxy_protocol_v2 helper, which parses it only when version 2 is enabled
and the source IP is valid. Connect failures use that exact text in the canonical
error redactor. Do not describe the PP2 parse as unconditional.

The locked Edgion Pingora revision 1aaa45c exposes BasicPeer::new(&str), whose
implementation parses an Internet SocketAddr and unwraps. Its
new_from_sockaddr constructor is private, although BasicPeer fields are public.
The constructor initializes empty SNI and PeerOptions::new. Direct struct
construction would couple Gateway code to those defaults; a public structured
constructor would require a separately scoped fork change. This task authorizes
neither implementation nor dependency changes.

## Equivalence boundaries

The textual round trip is not identical to copying every field of an arbitrary
structured address. A local std probe constructed IPv6 loopback port 443 with
flowinfo 123 and scope_id 7. Display produced [::1%7]:443; parsing retained scope
7 but returned flowinfo 0. This establishes a representation boundary, not a
claim that ordinary endpoint publication currently supplies nonzero flowinfo or
that a live connection is defective. A behavior-preserving replacement must
retain current normalization, or a deliberate behavior change needs approval.

TLS structured logs are populated before validate_endpoint_backend_addr rejects
non-Internet enums. Pingora renders a Unix pathname as text; a pathname shaped
like an Internet socket address can therefore produce log IP/port even though
the original enum is denied. Blindly replacing the prevalidation parse with
as_inet would change that diagnostic behavior. No Unix upstream support or
validation reordering is proposed.

PP2 currently receives the full bracketed IPv6 authority, not a reconstructed
bare IP plus port. Preserve destination bytes, source handling, authority TLV,
version gates, fallback behavior, writes/flushes and terminal accounting. Address
text also participates in exact known-address redaction in nested connect errors.
Removing it without reviewing those consumers can change sensitive-data handling.

## Alternatives and decision

- Retain the current handoff pending scope and measurements. This is temporary
  deferral, not a permanent exemption for repeated hot-path parsing.
- Introduce one canonical Gateway L4 peer-construction boundary consuming the
  validated structured address, with explicit current normalization and exact
  PeerOptions defaults. Share it across TCP, TLS termination and passthrough;
  do not duplicate constructors or add a parallel endpoint safety policy.
- Separately review an exact Pingora fork API change exposing structured
  construction. Repository policy still requires the Edgion fork, coherent
  dependency pins and independent verification of that expanded scope.
- Reuse an established typed destination for TLS log/PP2 consumers where byte
  equivalence is proven. Preserve prevalidation diagnostics and keep lazily or
  eagerly owned address text wherever current logs/redaction still need it.

Measure connection rate, retry count, PP2 frequency and address formatting/parsing
cost before claiming CPU or throughput gains. Avoiding a parse does not imply
removing the context/error address String or changing allocation count. Do not
cache whole mutable peer options merely to avoid parsing an address.

## Acceptance for an approved implementation

Cover IPv4, IPv6, scope IDs, nonzero flowinfo, unsafe addresses, non-Internet
enums, failed attempts and multiple retries. Preserve safety-guard ordering,
selected backend ownership, outlier completion tokens, connect timeout, TCP
keepalive, empty SNI/plain upstream TCP behavior and current retry policy.

Verify exact PP2 bytes and the disabled/invalid-source/write/flush outcomes.
Retain original-address logging on denied attempts and redaction across repeated
nested errors. Test both TLS modes and TCP through the shared implementation,
including new-connection configuration reload behavior. Preserve unrelated
listener-log and relay contracts. Do not add request tracing or metrics.

Original knowledge:
[TCP ownership](../../skills/04-review/routing/regression-guards/tcp-connection-snapshot-ownership.md),
[PP2 destination](../../skills/04-review/tls/regression-guards/pp2-dst-bare-ip.md),
and [address redaction](../../skills/04-review/tls/regression-guards/tls-log-address-redaction.md).
