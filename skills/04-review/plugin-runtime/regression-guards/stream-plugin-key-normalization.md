---
name: stream-plugin-key-normalization
description: Historical review evidence for Redundancy / L4 route handlers; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Stream plugin key normalization

## Current conclusion

`refs::plugin_attachment_fields::StreamPluginAttachment` borrows the actual Gateway,
TCPRoute, TLSRoute or UDPRoute and owns stream annotation normalization. Gateway
cold inputs and route units call the thin `routes::common::resolve_stream_plugin_store_key`
projection with this actual field, not detached ObjectMeta. Controller Gateway consumes
the same facade before status lookup and reverse-dependency registration. The former
metadata-only resolver consolidation is historical; do not restore that public seam.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `plugins-stream-F002` — 2026-05-26.

## Actual stream attachment owner boundary (C2)

The actual-owner facade is canonical; the Gateway common helper only maps its result into the existing runtime representation. Invalid route owners are independently rejected by the pre-existing backend identity gate before route-unit construction. Keep actual-resource projection tests and publication decoy guards. See the [canonical stream attachment contract](../intentional-design/stream-plugin-store-key-normalization.md).
