---
name: kafka-broker-dialing-sdk-owned
description: Use when reviewing Kafka LinkSys broker DNS, metadata-discovered destinations, or canonical egress-policy coverage.
knowledge_type: intentional-design
finding_id: kafka-broker-egress-dns-policy
---

# Kafka broker dialing remains SDK-owned without in-process egress classification

## Conclusion and scope

The Gateway Kafka LinkSys provider intentionally leaves bootstrap and
metadata-discovered broker DNS resolution and TCP dialing to `rskafka` 0.6.
These dials do not use Edgion's `EgressResolver`, do not enforce its unconditional
loopback/link-local/unspecified rejection, and do not follow
`outbound_egress_block_private`.

This is an explicitly accepted, Kafka-only exception for an operator-created
infrastructure connector. It is not a claim that the path is protected, fixed, or a
false positive, and it does not waive the canonical dial-time egress rule for new
providers or request-influenced destinations. Deployments that need destination
containment must enforce it outside the process, for example with Kubernetes
NetworkPolicy or equivalent node/network controls.

## Rationale and evidence

`KafkaClientConfig.brokers` flows through
`KafkaLinkClient::build_kafka_client` into `rskafka::client::ClientBuilder`.
`rskafka` then owns both bootstrap connections and brokers learned from Kafka
metadata. In version 0.6, its private `BrokerConnector` sends both forms through
`Transport::connect`, which ultimately calls `tokio::net::TcpStream::connect`
without a public resolver or dialer injection point.

A bootstrap-only validation or pre-resolution step would leave advertised brokers,
leader changes, and reconnects outside the check. A complete in-process correction
therefore requires changing the dependency boundary, most directly by adding a
broker dialer hook to `rskafka`. The project declined to maintain a fork solely for
that hook. Avoiding that ongoing dependency cost is the accepted benefit; the cost is
that a configured hostname, DNS answer, or broker metadata response can direct the
Gateway to an address that canonical egress policy would otherwise reject.

The decision assumes Kafka LinkSys resources and their clusters are trusted
operator-managed infrastructure. Stable broker addresses reduce operational DNS
churn but do not reduce the security consequence of a single disallowed address, so
address stability is not itself the acceptance rationale.

## Alternatives and limits

- Do not present configuration-time validation, IP-literal-only checks, or bootstrap
  pre-resolution as equivalent mitigations; none covers metadata-discovered brokers.
- Do not replace broker hostnames with resolved IPs. That freezes DNS and can break
  TLS SNI and hostname verification.
- Switching to `librdkafka` or `rust-rdkafka` is not a narrow fix: it changes the
  dependency/runtime model, producer lifecycle, and TLS integration.
- Routing Kafka output through an external collector is a valid future product
  simplification, but it removes or changes the direct Kafka LinkSys contract and was
  not selected for this closure.

## Verification and re-evaluation

The closure made no runtime, schema, test, user-documentation, or canonical-design
change. Current source and the dependency's connection path were inspected before the
tradeoff was accepted; historical or future checks must not be represented as proof
that the missing guard is harmless.

Re-evaluate this decision when any of the following occurs:

- `rskafka` exposes a resolver or broker-dialer hook that covers bootstrap and
  metadata-discovered connections without a maintained fork;
- the Kafka client dependency is replaced or otherwise receives a low-cost safe
  connector extension;
- Kafka destinations become influenced by request data or a less-privileged tenant;
- the product supports untrusted or externally operated Kafka clusters;
- Edgion promises uniform in-process egress enforcement for every LinkSys provider;
- an incident demonstrates that network-level containment is insufficient.

## Resolution references

- `kafka-broker-egress-dns-policy`, accepted tradeoff on 2026-09-13: no code fix;
  maintaining an `rskafka` fork solely for complete dial-time enforcement was
  declined.
