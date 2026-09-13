---
name: tls-last-known-good-follows-resource-key
description: Use when reviewing EdgionTls last-known-good retention across invalid updates, delete/recreate coalescing, reconnect full snapshots, or proposals to bind retention to metadata.uid.
knowledge_type: intentional-design
---

# EdgionTls last-known-good follows the resource key

## Conclusion and scope

`TlsStore` intentionally retains a valid EdgionTls predecessor for an invalid
update or reconnect snapshot at the same namespace/name key, without requiring
the incoming resource to have the same `metadata.uid`. The availability policy
is tied to the configured TLS slot, not to the Kubernetes object lifetime.

An explicit partial `add` still starts a new generation without a predecessor,
an absent key in a full snapshot is removed, and an empty full snapshot clears
the store. Invalid attachment dimensions also revoke old matcher admission.

## Rationale and evidence

The accepted product priority is continuity of the last usable certificate. If
a resource is deleted and recreated at the same key with invalid certificate
material, continuing to serve the previous valid certificate is preferable to
withdrawing TLS service; a later valid generation replaces it normally.

`TlsStore::validated_entry` therefore accepts the current same-key entry as the
last-known-good predecessor when its certificate is valid and the incoming
attachment dimensions are bounded. `replace_all_tls` obtains that predecessor
from the current entry map by namespace/name. `apply_partial_update` passes a
predecessor for updates but deliberately passes `None` for explicit adds.

`resourceVersion` and the Edgion sync version continue to order and correlate
configuration changes. They are not retention identities: both advance on
ordinary same-object updates, while reconnect snapshots and coalesced workqueue
events do not preserve enough event history to distinguish every delete/create
sequence from an update.

## Alternatives and limits

Binding retention to an equal Kubernetes UID was rejected. It would distinguish
same-object updates from delete/recreate, but it would also withdraw a usable
certificate when the replacement is temporarily invalid and would make
UID-less file-system or etcd resources fail closed on every invalid update.

The accepted cost is that an invalid resource recreated under the same key may
temporarily serve certificate identity, hostname/listener membership, and TLS or
mTLS behavior from the deleted generation. This is intentional continuity, not
proof that the incoming generation was accepted. Operators must publish a valid
replacement or remove the key from the authoritative full snapshot to withdraw
the retained configuration.

This decision does not weaken certificate parsing, hostname or attachment
validation, Secret resolution, or runtime expiry checks. It adds no permission
to retain across a different namespace/name key.

## Verification and re-evaluation

Current unit coverage includes invalid-update retention, reconnect/full-snapshot
retention, explicit-add replacement, absent-key removal, empty full-set clearing,
valid recovery, and rejection-log rearming. During the 2026-09-13 review,
`cargo test -p edgion-gateway --lib tls::store::tls_store::tests` passed before
the rejected UID-gating experiment was removed; final repository checks are
recorded in the issue closure report rather than claimed here in advance.

Re-evaluate if the product changes from availability-first continuity to strict
object-lifetime isolation, if invalid recreated resources must withdraw old TLS
identity immediately, or if conf-sync gains an authoritative lifecycle marker
whose semantics apply consistently to partial events and reconnect snapshots.

## Resolution references

- `tls-last-known-good-resource-identity` — accepted tradeoff on 2026-09-13;
  UID-bound retention was declined in favor of same-key continuity.
