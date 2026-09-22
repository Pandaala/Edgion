---
name: downstream-keepalive-request-limit-default
description: Use when reviewing the default value or CPU scaling of Edgion's downstream HTTP/1 keepalive request limit.
knowledge_type: intentional-design
---

# Downstream HTTP/1 keepalive defaults to 10,000 requests per connection

## Conclusion and scope

`EdgionGatewayConfig.spec.server.downstreamKeepaliveRequestLimit` defaults to
`10000`. The value is a fixed per-connection HTTP/1 request budget and is not
multiplied by the Gateway worker or CPU count. Operators can set it to `0` for
unlimited reuse or choose a smaller value when they want more frequent connection
rotation.

## Rationale and evidence

The Pingora server API exposes this limit as an optional per-connection guard and
defaults to no limit. Envoy and Traefik also default their corresponding request
count limits to unlimited. A finite Edgion default is therefore a conservative
rotation policy, not a requirement for ordinary request-memory reclamation.

Pingora creates a new HTTP/1 request session for each reuse, so request-scoped
state is released independently of the transport connection. A finite limit can
still periodically release TCP/TLS connection state and retained buffer capacity.
The `10000` default keeps that safeguard while avoiding the relatively frequent
rotation imposed by the former `1000` default.

CPU scaling is rejected because the field limits each individual connection.
Multiplying it by worker count would make every connection live longer on larger
machines; it would not express a process-wide capacity budget.

## Alternatives and limits

The former `1000` default follows Nginx but has no Edgion-specific workload or
memory evidence showing that such frequent rotation is necessary. An unlimited
default would align directly with Pingora, Envoy, and Traefik, but gives up the
defensive periodic release of connection-level high-water allocations. Values
such as `100000` are effectively unlimited for many workloads and provide less
meaningful protection.

The limit does not bound concurrent connections, file descriptors, total process
memory, or HTTP/2 connection lifetime. The separate downstream keepalive idle
timeout still closes inactive HTTP/1 connections.

## Verification and re-evaluation

The schema default, Gateway fallback snapshot, CRD default, and user-facing
documentation must remain equal. Re-evaluate the number using production memory
profiles and connection-churn measurements, or if Pingora changes which state is
retained across HTTP/1 request sessions.

## Resolution references

- `http-keepalive-request-limit-counting` follow-up default decision — 2026-09-21.
