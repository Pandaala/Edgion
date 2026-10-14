---
name: structured-file-init-envelope
description: Use when reviewing FileSystem initialization kind detection, YAML/JSON envelopes, or scan dispatch.
knowledge_type: regression-guards
---

# File initialization selects the structured root kind

## Conclusion and scope

FileSystemWatcher::prime uses the Resources envelope helper followed by
ResourceKind::from_kind_name. Quoted YAML root keys, flow mappings, and JSON
field order must not redirect or suppress an otherwise valid resource's
initialization event. Nested kind fields do not classify the containing resource.
The same path runs when a new watcher generation is created on reload.

This is classification, not typed acceptance. Filename identity owns PathRegistry
keys; the resource controller still checks content identity, supported source
versions and the typed schema. Runtime PollWatcher events continue to dispatch
by filename. Kubernetes watches are outside this correction.

## Rationale and evidence

ARCH-047 found that ResourceKind::from_content searched the whole document with
JSON/YAML regexes. A quoted YAML root key could be missed, while a nested JSON
Service kind preceding the HTTPRoute root kind selected the wrong channel.
Empty Init/InitDone still made the intended kind ready, so downstream validation
could not recover the missing InitApply.

The lexical method is removed. `resource/meta/envelope.rs` already owns structured
extraction for Admin, CLI and FS storage. The watcher now consumes that owner;
there is no additional scanner or generic dispatch mechanism. See
[FS architecture](../../../01-architecture/01-controller/03-config-center/01-file-system.md)
and [resource envelope ownership](../../../01-architecture/00-common/03-resource-system.md#untyped-resource-envelope).

## Alternatives and limits

Direct reuse materializes a transient Value during each cold scan lookup and
releases it before returning. No opaque tree is retained or cloned into the
snapshot. A lightweight visitor could reduce transient allocations, but would
need to preserve YAML aliases/tags, non-string values, duplicate-key handling and
JSON fallback. No measured scan cost justifies that additional semantic owner.
Reconsider only with profiling and review of the shared envelope boundary.

The existing helper requires metadata presence, not object shape. It attempts
YAML first and falls back to JSON on parse failure. Missing/non-string kind fields
remain absent; unknown kind names fail mapping. Malformed documents and missing
metadata now stop at scan admission, before typed processing. This does not
promise status sidecars for files rejected before dispatch. Existing aliases and
case-insensitive name mapping are retained, without relaxing typed source checks.

## Verification and re-evaluation

The mirrored watcher unit tests drive real tempfile prime/dispatch channels:
`structured_root_kind_controls_init_channel`,
`invalid_init_envelopes_do_not_register_or_dispatch`, and
`repaired_structured_envelope_reloads_with_filename_collision_identity`.
They cover exact InitApply channel/content, empty alternate channels, scan silence,
filename-owned collisions, repaired/repeated generations and snapshot release.
Existing phase, replay, document-release, typed-identity/API-version and
single-worker tests remain required. Envelope tests characterize metadata shape,
non-string kind, YAML aliases/tags and duplicate-key fallback without changing
the shared helper's executable code.

Fresh check results are recorded in the implementation record after execution;
these unit tests do not establish live Kubernetes or end-to-end gateway behavior.
Re-evaluate when envelope parsing, scan admission, filename identity, init phases
or parser dependencies change.

## Resolution references

- ARCH-047: structured resource-kind extraction; replaces lexical cold-scan
  classification with the existing Resources envelope owner.
