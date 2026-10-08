# Existing-capability baseline

## Meta

- Created: 2026-09-25
- Status: pending
- Type: config
- Depends on: None
- Branch: Not assigned
- Output: Pending

## AI guide summary

Establish a reproducible business Gateway deployment before adding runtime features.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Prepare isolated Controller/Gateway configuration with a fixed namespace list, distinct ownership, and two TLS-capable backend Services.
- Choose an explicit entry address and backend discovery arrangement without loops; configure ports before Gateway startup.
- Use disposable locally generated test certificates without cert-manager, existing BTP and frontend TLS resources, and existing HTTP/gRPC routes.
- Inspect current test/deployment infrastructure before adding fixtures; record exact commands and environment requirements.

## Acceptance

- [ ] HTTP and gRPC reach the intended backends through the Gateway.
- [ ] Gateway-to-backend TLS and mTLS succeed; missing client credentials and untrusted certificates fail as configured.
- [ ] A Secret update reaches all tested Gateway replicas and changes new handshakes without restarting them.
- [ ] Document frontend protection and direct-backend access separately; record namespace isolation and observed limitations.

## Out of scope

Automatic issuance, new production resource types, transparent capture, and deployment of real business credentials.

