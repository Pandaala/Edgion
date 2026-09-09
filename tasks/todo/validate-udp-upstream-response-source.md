# Validate UDP Upstream Response Source

## Meta

- Status: pending
- Type: bug
- Priority: P2
- Severity: medium

## Evidence and Acceptance

`routes/udp/edgion_udp.rs` binds a family-matched upstream socket without connect,
sends to the pinned endpoint with send_to, then discards recv_from's source and
forwards its bytes downstream. A different sender able to reach that ephemeral
socket can therefore supply forwarded bytes. Reachability, port discovery and
application-level validation constrain actual exploitation; no successful network
exploit or arbitrary valid application response is claimed.

- Plan first: define exact pinned IP/port semantics and choose connected sockets
  or receive-time validation with explicit IPv4-mapped IPv6 handling.
- Reject unapproved senders without success accounting or activity refresh.
- Preserve correct-source forwarding, IPv4/IPv6 behavior and selected destination
  safety checks; verify wrong IP and wrong port independently.
- Reuse existing mechanisms and add no per-packet tracing or unbounded labels.
- Add real socket/wire coverage only in a later authorized integration task;
  existing integration files remain frozen unless their exact paths are explicitly authorized.

Local ownership changes leave this policy unchanged and must not
claim to fix sender validation.
