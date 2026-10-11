---
id: ARCH-140
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-388
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---


# Keep authored values out of the Admin resource parser's diagnostics

## Current boundary and concrete witness

api/common.rs::parse_resource first tries serde_json and then serde_yaml. When the YAML attempt fails, it emits the complete serde_yaml::Error Display at WARN before returning 400. The shared helper is used for typed parsing on actual Admin create/update and validation paths, so rejection of an input is also a persistent logging surface.

An otherwise ordinary EdgionTls Admin document with spec.clientAuth.mode set to an unrecognized string demonstrates the boundary. ClientAuthMode in edgion-resources/src/resources/edgion_tls.rs is a derived closed enum. EdgionTls does not enter the Gateway API raw-schema or source-version gates. Namespaced update binds its namespace, then calls parse_resource_and_update_version at569–570 before schema validation and persistence. Its typed parse fails in parse_resource, whose warning prints the supplied mode string.

This conclusion follows from the actual pinned Serde implementation, without executing the input: serde_core1.0.229::de::Error::unknown_variant inserts the supplied variant verbatim, and serde_yaml0.9.34's custom error retains and displays that message. An authored private configuration value placed in that malformed field is therefore copied into the warning. Other type/value failures can carry input values too. This does not claim that a valid credential field is logged on every request, that an unauthenticated caller reaches this handler, or that a production leak was observed.

The structural problem is that a parser shared by many typed resource owners exposes a library diagnostic directly as a safe log summary. Adding a new enum or typed field changes the values this logging path can reveal, although the consumer merely needs to report which admission stage failed. Resource-output serde redaction runs on successful serialization and cannot protect an error already emitted during deserialization.

## Bounded improvement and preserved contracts

Give this existing Admin parsing owner a value-free diagnostic boundary. Keep a fixed parse-stage classification and numeric line/column from Error::location when present; do not interpolate Error Display, the body, or the parser's raw authored-key path. A private formatter or fixed structured fields here is sufficient. No exception-text scrubber, global redaction registry, resource taxonomy or copied request-body owner is necessary.

Inspect the adjacent source-version warning when applying the change: it also receives a serde_yaml error and must not reintroduce an unrestricted Display for the same authored document. Preserve useful existing operation context without logging the malformed value. Do not add another warning or metric. This recommendation concerns logs; authorized validation callers' existing verdict shape need not be redesigned.

Preserve JSON-first/YAML fallback, accepted documents, source-GVK checks and canonical Gateway API normalization, namespace admission and immutable identity merging, opt-in update preconditions, original raw HTTPRoute forwarding, schema placement and the synchronous 400-before-write contract. Do not change Controller authn/authz, last-known-good reload policy or introduce handler I/O. A logging-only correction must leave all current parsing and policy decisions unchanged.

## Existing evidence, decisions and future verification

The complete common_test.rs and cluster update-identity tests were read. They cover source-version conversion/rejection, status mapping and precondition precedence, while the relevant actual handler bodies establish the typed-before-schema call. They do not assert value-free parser log fields. Actual cache-redaction tests cover successful output and transport separation, a different surface.

Future verification should capture the actual parser warning for JSON and YAML documents with an unrecognized ClientAuthMode marker and a malformed scalar type, proving the marker and authored keys are absent while status400 and useful numeric location remain. Include valid controls to retain parsing behavior, and drive one real Admin handler case to show no persistence occurred. No tests or application were executed in this review.

ARCH-089 concerns jsonschema ValidationError formatting after a successful typed parse. Its formatter cannot repair this earlier failure, and this proposal does not repeat schema validation or broaden that finding. Prior parser/YAML-view and ingress findings were searched; no matching pending implementation was found.

Original log-safety rules forbid leaking configuration or sensitive values. The config-client-data-preview-log-safety knowledge page correctly records removal of explicit body previews, but its assertion that serde_yaml error Display does not expose values is too broad against the pinned implementation. Its accepted no-body-preview correction is retained; that assertion should be narrowed when implementing this finding. The other function's latent reachability and historical acceptance are not evidence that this live Admin parse warning is safe. No knowledge file was changed during this read-only review.
