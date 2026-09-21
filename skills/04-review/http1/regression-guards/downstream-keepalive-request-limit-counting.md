---
name: downstream-keepalive-request-limit-counting
description: Use when reviewing EdgionGatewayConfig downstream HTTP/1 request limits, Pingora reuse budgets, or ordinary and redirect listener keepalive behavior.
knowledge_type: regression-guards
---

# Downstream HTTP/1 keepalive limits count total requests, not later reuses

## Conclusion and scope

`EdgionGatewayConfig.spec.server.downstreamKeepaliveRequestLimit` is the maximum
total number of requests served by one downstream HTTP/1 connection. A value of
`0` is unlimited, `1` permits only the first request, and `2` permits one later
reuse. The field does not limit HTTP/2 connections.

The first request captures the current resolved value for its connection. Later
requests on that connection retain the captured budget across EGC updates. Both
the ordinary proxy and HTTP-to-HTTPS redirect listener use the same conversion.

## Rationale and evidence

Pingora's `HttpPersistentSettings::apply_to_session` decrements
`keepalive_reuses_remaining` when constructing the next HTTP/1 session, and its
HTTP/1 codec refuses another reuse when that value is `Some(0)`. The Pingora value
therefore counts later reuses, while Edgion's public field counts the first request
as part of the total.

`routes/http/proxy_http/pg_early_request_filter.rs::apply_client_policy` converts
the public total with `request_limit.checked_sub(1)`: zero becomes `None`
(unlimited), one becomes `Some(0)`, and every larger value becomes the exact
number of later reuses. `EdgionHttpProxy` and `EdgionHttpRedirectProxy` persist an
initialization marker through Pingora's connection user context so a reused
connection cannot replace its captured budget with a newer EGC generation.

## Alternatives and limits

Passing the configured value through unchanged is incorrect because it permits
the first request plus that many reuses. Changing the public field to mean
"reuses after the first request" is also rejected: it contradicts the resource
schema and operator documentation, and makes a configured value of one unlike
the established Envoy and Nginx total-request convention.

Other conditions may close a connection earlier, including a client
`Connection: close`, invalid HTTP/1 framing, graceful shutdown, body-consumption
failures, and response-path reuse decisions. The configured limit is an upper
bound, not a guarantee that the connection reaches that many requests.

## Verification and re-evaluation

The ordinary-listener wire regression drives Pingora's HTTP/1 codec, persistent
settings, Edgion connection context, and response framing for limits `0`, `1`,
and `2`. It observes three requests before an explicit client close for unlimited,
one response for limit one, and two responses for limit two, including the
corresponding `Connection` headers. Redirect tests verify the same `0`/`1`/`2`
mapping and execute the listener's early policy in its existing response-wire and
stream-reuse coverage.

Focused Gateway unit tests passed on 2026-09-21. Re-evaluate if Pingora changes
the meaning or decrement point of `keepalive_reuses_remaining`, Edgion adds an
HTTP/3 ingress path, or the public field is extended beyond HTTP/1.

## Resolution references

- `http-keepalive-request-limit-counting` — fixed on 2026-09-21; the prior
  pass-through permitted `N + 1` requests for every nonzero configured limit.
