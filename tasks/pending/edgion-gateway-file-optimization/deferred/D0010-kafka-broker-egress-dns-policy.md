# D0010 — Enforce Kafka Broker Egress and DNS Policy

## Meta
- Status: needs-investigation
- Severity: high
- Discovered in: B0048, `edgion-gateway/src/link_sys/providers/kafka/client.rs`
- Category: security

## Evidence
Operator-configured bootstrap brokers are passed directly to `rskafka::ClientBuilder`, and broker addresses discovered from Kafka metadata remain SDK-controlled. This path does not visibly apply Edgion's canonical dial-time `EgressResolver`, IP classification, or operator `ResolverSpec` policy.

## Why deferred
A correct solution depends on rskafka connector and resolver extension points and must also cover advertised brokers. Validating strings once at configuration time would not enforce per-dial DNS and address policy.

## Impact and open questions
- Broker DNS answers or advertised addresses may bypass canonical egress classification.
- Kafka cluster discovery introduces destinations beyond the bootstrap list.
- Private broker networks and resolver configuration need an explicit product contract.

## Candidate next step
Investigate rskafka connector, DNS, and metadata address hooks. Design dial-time enforcement for bootstrap and advertised brokers using Edgion's canonical egress, TLS, and DNS mechanisms before implementing a provider change.
