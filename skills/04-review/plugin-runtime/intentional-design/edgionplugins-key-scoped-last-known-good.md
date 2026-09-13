---
name: edgionplugins-key-scoped-last-known-good
description: Use when reviewing EdgionPlugins DSL/WAF last-known-good retention across updates, same-name recreation, full relists, or compressed config events.
status: accepted
finding_id: D0030
closed: 2026-09-13
knowledge_type: intentional-design
---

# EdgionPlugins last-known-good is scoped by logical resource key

## Conclusion and scope

The Gateway intentionally treats an EdgionPlugins `namespace/name` as the
logical configuration identity for DSL/WAF last-known-good retention. If a
resource with that key remains present but its latest DSL or native WAF program
is invalid, the publisher may continue executing the last activatable runtime
for the key. This remains true when Kubernetes reports a different
`metadata.uid`, when a reconnect supplies a full snapshot, or when
delete/recreate events are compressed into an update.

An explicitly observed removal still clears the prior candidate. An empty full
snapshot clears all candidates. An invalid resource with no prior activatable
generation publishes the sealed fail-closed runtime. Retention also remains
subject to the current GatewayClass plugin policy and the existing security
guards: an enabled JWE entry or an executable cross-namespace RequestMirror in
either generation prevents whole-resource retention.

This is a key-scoped availability choice, not a claim that Kubernetes UID is
unavailable or that a recreated object is the same Kubernetes object. The
Controller and Gateway deliberately preserve service continuity for the logical
configuration slot rather than making UID change an unconditional revocation
boundary.

## Rationale and evidence

`CacheData::reset` publishes List results through `ConfHandler::full_set`, and
`PluginStore::full_set` forwards the map to
`PluginPublisher::replace_http_plugins`. The publisher keys `http_sources`,
`http_latest`, and prior selections by `namespace/name`. A changed source gets a
new process-local generation, but `Inputs::compose` may select the earlier
executable generation when the latest compilation requests
`InvalidUpdatePolicy::RetainPrevious`.

`sync_version` orders configuration events and causes changed input to be
processed; it is not a stable resource identity. A larger value therefore does
not distinguish an update of one object from deletion followed by same-name
creation. Kubernetes `metadata.uid` can make that distinction for Kubernetes,
but FileSystem and etcd do not have the same source-owned identity contract.
Using the common logical key keeps retention semantics consistent across all
three configuration providers.

The runtime event path does not provide a stronger general identity boundary.
`ResourceProcessor::save` emits `ResourceChange::EventUpdate`. Gateway event
compression classifies a final present value as an add only if the accumulated
events contain `EventAdd`; otherwise a compressed delete/update is dispatched as
an update. Whether removal was observed separately therefore affects whether the
old candidate was cleared. That history dependence is accepted as part of this
last-known-good availability model.

Current source and colocated tests establish the implementation shape:

- `conf_handler_impl::invalid_partial_update_retains_last_known_good` covers an
  invalid DSL update retaining the prior executable entry.
- `conf_handler_impl::invalid_same_key_add_replaces_last_known_good_and_fails_closed`
  covers the explicit fresh-add API boundary.
- `conf_handler_impl::invalid_full_snapshot_retains_last_known_good_but_drops_absent_keys`
  covers key-scoped full-snapshot retention and removal of omitted keys.
- `plugin_store::waf_store_publishes_atomically_and_pins_in_flight_generations`
  covers WAF retention, later recovery, removal, and in-flight generation pins.

## Alternatives and limits

The accepted benefit is continuity when an operator submits an invalid DSL/WAF
generation or when reconnect/relist loses incremental event history. A malformed
generation does not replace a working request path merely because its resource
version is newer.

The accepted cost is that a deleted and recreated resource with the same key may
temporarily execute the deleted generation until a valid generation is
published or a removal is observed independently. Because retention is for the
whole compiled resource, this can also preserve non-JWE plugin behavior from the
prior generation, including authentication, authorization, rate-limit, WAF, and
request/response mutation behavior. This risk was explicitly accepted for
D0030 on 2026-09-13; it is not described as fixed or disproved.

A cross-provider object-identity contract was rejected for this issue. A
Kubernetes-only UID comparison would make behavior provider-dependent, while a
new persisted identity for FileSystem and etcd would add protocol and lifecycle
complexity without changing the chosen key-scoped product semantics.

The separate D0031 proposal to add an integration scenario that requires
UID-bound retention remains closed. Such a scenario would assert behavior that
the product does not require. Existing tests for explicit add, update, full-set,
removal, recovery, policy enforcement, JWE, and mirror boundaries remain useful;
no new identity-specific integration scenario is required by this decision.

## Verification and re-evaluation

During the D0030 review on 2026-09-13,
`cargo test -p edgion-gateway --lib invalid_full_snapshot_retains_last_known_good_but_drops_absent_keys -- --nocapture`
passed with one test. Source inspection covered `ResourceProcessor::save`,
Gateway `CacheData` event compression, `PluginStore` full/delta handlers,
`PluginPublisher::{replace_http_plugins,update_http_plugins}`, and retained
candidate selection in `Inputs::compose`. Full repository closure checks are
recorded in the closing response rather than predeclared here.

Re-evaluate this choice if:

- an incident shows materially harmful continued execution of a deleted
  authentication, authorization, WAF, or mutation policy;
- a new plugin category carries security state that must join the JWE or
  cross-namespace mirror non-retention boundary;
- Edgion adopts a canonical stable resource identity across Kubernetes,
  FileSystem, and etcd for another required mechanism;
- product requirements make deletion an immediate revocation boundary even
  when reconnect or event compression hides the standalone delete; or
- full-snapshot and compressed-event history dependence causes an operational
  correctness problem that outweighs last-known-good availability.

## Resolution references

- `D0030` / `tasks/todo/edgionplugins-last-known-good-resource-identity.md` —
  accepted key-scoped availability tradeoff on 2026-09-13; no identity contract
  implemented.
- `D0031` / `tasks/todo/add-edgionplugins-lkg-identity-integration-coverage.md` —
  closed on 2026-09-05 because identity-specific coverage was not independently
  actionable.
