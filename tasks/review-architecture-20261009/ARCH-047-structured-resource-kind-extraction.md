---
id: ARCH-047
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-198
analyst: /root
independent_verification: confirmed by /root/analyze_outbound
---

# Read resource kind from the structured top-level envelope

ResourceKind::from_content in resource/kind.rs:120-142 first searches the entire
input for a JSON-looking kind pair, then for a bare YAML kind key. It does not
parse a top-level field. FileSystemWatcher::prime:165-184 uses this result to
skip a file or choose its initialization channel before any typed resource
validation. The same Resources crate already owns structured top-level metadata
extraction in resource/meta/envelope.rs, used by Controller admin/storage and CLI.

A valid YAML document with the root key written as 'kind': HTTPRoute has no
matching bare kind key or JSON-looking pair. A correctly named existing
HTTPRoute_default_route.yaml is skipped during prime. No InitApply reaches its
HTTPRoute processor, while dispatch_init still emits empty Init/InitDone and
allows that kind to become ready.

A second witness uses a valid JSON HTTPRoute with spec before the root kind:
{"spec":{"parentRefs":[{"name":"gw"}],"rules":[{"backendRefs":[{"name":"backend","kind":"Service","port":80}]}]},"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route","namespace":"default"}}.
The first regex captures nested Service and groups the file under Service,
rather than HTTPRoute. A YAML file can likewise contain JSON-style nested
mapping pairs; the JSON regex runs before the root YAML search. Field order or
syntax style changes routing of the same document. Subsequent typed validation
cannot restore an InitApply that never reached the correct channel.

The current runtime watcher chooses the channel from the parsed filename, so a
later source modification has a different dispatch path. This finding concerns
initialization and reload scans; it does not claim that every later update fails,
that wrong-kind content is successfully published, or that filename/content
identity, API-version and schema gates can be bypassed. Witnesses are derived
from inspected source; no parser or controller reproduction was executed.

Replace lexical kind detection with the canonical structured envelope at this
cold source-admission boundary, and remove the unused lexical path once its sole
production caller is migrated. Select only the top-level string kind and retain
the existing ResourceKind name mapping. If a lighter projection is necessary,
put it behind the same envelope owner and share parsing semantics; do not add
another regex scanner or a new generic resource-dispatch framework.

Preserve valid YAML/JSON, current case-insensitive names, malformed/unknown input
rejection, authoritative filename identity, deterministic collision winners,
private complete PathRegistry publication, phased init, single worker and
current supported API-version conversions. A missing metadata object remains
invalid for real resources. Structured detection supplements typed validation;
it does not replace it. Avoid retaining/cloning full opaque spec trees merely
for this lookup, and keep parsing off Gateway request paths. No raw resource or
secret contents should enter diagnostics.

Original resource-system/add-resource and FileSystem architecture, review routing,
canonical inventory and status-version guards were checked. Their manual wire
enum and filename contracts do not require lexical content classification.
Existing watcher tests cover plain bare-key files, phased init and repeated
watcher generations; kind tests focus wire value 23, while envelope tests cover
ordinary structured YAML/JSON. No retrieved decision accepts syntax-dependent
top-level kind detection. Source parsing and alias mapping are distinct from
earlier ARCH findings.

Future verification should exercise the actual prime/dispatch path with quoted
YAML keys, valid JSON with nested kind before the root kind, root-field order
changes, malformed/unknown kind and normal files. Verify exactly the intended
InitApply channel, preserved registry identity/collision behavior, phased
readiness and repaired reload generation. Use tempfile and in-process channels;
no integration topology or request-path abstraction is needed. Exact primary and
support hashes/scopes are in REPORT-PART-198.json. Static read-only review only;
no builds, tests, network, Git mutations or production changes. Independent
verification is pending.
