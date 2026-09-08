---
name: initial-documents-consumed-by-phase
description: Review FS and etcd startup document retention across phased initialization and runtime entry.
knowledge_type: regression-guards
---

# Initial resource documents are consumed by phase

## Conclusion and evidence

D0036 found FS and etcd watchers retaining full initial raw documents in `primed`
throughout runtime, even after Secret rotation/deletion. Dispatch cloned the
requested vectors and each outgoing document. These copies were independent of
the authoritative raw store and sync history and had no runtime consumer.

`FileSystemWatcher::dispatch_init` and `EtcdWatcher::dispatch_init` now clone sender
handles before locking the snapshot, remove only requested subscribed kinds, and
move their document contents into events. Later phases and unsubscribed kinds are
retained until their dispatch; empty kinds still receive Init/InitDone. Runtime
entry drops leftover initial documents. Etcd retains its revision and known keys;
FS retains its file index and does not gain a prime precondition for runtime.

## Lifecycle and limits

`non_kube_common::spawn_all_controllers` subscribes first and dispatches disjoint
provider, Gateway, and dependent phases. No sync guard crosses an awaited send.
Same-kind snapshot replay after consumption is not a supported startup contract;
cancelled startup restarts via re-prime/new watcher. Source files, etcd/K8s storage,
event queues, raw/processed stores and active generations remain independent owners.
Dropping initial strings is not erasure of their allocator storage.

## Verification and re-evaluation

Both watcher modules test `init_documents_release_per_subscribed_phase`, covering
payloads, phase isolation, late subscription and empty kinds. Runtime release tests
cover FS leftovers and etcd revision/known-key preservation without network IO.
Re-evaluate when startup phase membership, replay/cancellation, subscriptions, or
runtime snapshot consumers change. Final execution results are in the
[credential ownership audit](../../security/review-evidence/credential-copy-ownership.md).
