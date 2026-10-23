# Evaluate decision-scoped RealIp trust resolution

Status: deferred user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `79f8be518`. No runtime implementation selected.

## Evidence

RealIpPolicy::resolve_session and resolve_in first check complete availability of
the actual-owner-bound trusted references, then pass a membership closure into
RealIpExtractor. The extractor checks the downstream peer once and checks each
valid visited forwarded hop as required by recursive/nonrecursive policy.
BoundIpPredicate uses ordered all-reference availability and any-reference membership.

EdgionConfigDataSnapshot::ip_list_matcher performs denied-scope rejection, key
splitting, raw namespace/name identity, enable/type/visibility checks and typed-index
lookup. Availability invokes that reader once per visited reference; each subsequent
peer/hop membership invokes it again for each reference visited before a match.
The existing snapshot helper already avoids a second typed-index lookup within a
successful scalar read. The remaining candidate is reuse across one complete trust
decision, not that already-fixed duplicate lookup.

Current reads already use one immutable captured generation. There is no repeated
ArcSwap capture or matcher compilation per hop, and individual scalar reads do not
allocate a result vector. The plugin path uses object-safe session capabilities;
the global path passes the captured snapshot directly. Do not describe this as a
current generation-consistency defect or assume a new cache is necessary.

## Authority and design decision

Any unavailable reference disables the whole RealIp trust decision. A missing,
disabled, denied, wrong-type or unreadable sibling cannot be ignored merely because
an earlier list contains the peer. Preserve all-reference preflight before accepting
membership. Invalid owner bindings and denied scopes retain their early rejection;
Required and Partial condition roles must not be merged into Trust behavior.

The original plugin-system architecture keeps resource/snapshot loans out of
PluginSession. The ConfigData publication knowledge also explicitly avoids matcher
vectors in consumers. Those are current boundaries, not permissions this task can
silently relax. Consider:

- Retain the existing lazy scalar reads when short chains and small reference sets
  make repeated lookup cheaper than additional temporary storage or dispatch.
- Investigate a narrow scalar whole-decision capability under the canonical owner,
  without exposing resource loans to plugins. Establish whether it actually reduces
  lookup work before adding another interface.
- Consider internal decision-scoped borrowed matcher storage only as an unapproved
  architecture alternative. It needs independent justification and reconciliation
  with the existing no-vector/no-loan guidance before implementation. It must retain
  all-or-nothing availability and cannot reuse the ACL iterator's filter_map behavior
  to silently skip an unavailable trust reference.

Never pin live matchers to a policy generation across later requests: list updates,
visibility changes, deletion, replacement and recovery must continue to affect new
decisions. Reuse may live only within the exact captured snapshot's lifetime. Keep
actual-owner permission proofs; direct raw key lookup is not an authority shortcut.
No global cache, second matcher implementation, new reference limit or per-hop logging
is selected. Reference collections are Vec-backed; justify temporary capacity and
spill behavior rather than assuming a small fixed count.

## Acceptance and measurements

Compare complete decisions for unavailable siblings before/after a positive match,
denied and stale owner proofs, cluster-only visibility changes, malformed raw
identity, missing/disabled/wrong-type targets, empty/invalid reference input and
same-policy live deletion/recovery. Retained old snapshots must remain internally
consistent while subsequent decisions observe newly published generations.

Preserve right-to-left traversal, invalid-token skipping, recursive and
nonrecursive behavior, hop caps including zero, all-trusted fallback, peer parsing,
IPv4/mapped-IPv6 representation, header selection and global XFF aggregation.
Keep fixed diagnostic categories, sensitive logging, setter invocation behavior
and the separate ip-02 Display decision unchanged.

Measure one/many references, early/late membership hits, misses, short/long chains,
no header and untrusted peers. Report raw/index lookups, matcher calls, temporary
allocation/capacity, Arc ownership and CPU separately. Preparing all state eagerly
can regress short paths even if it reduces long-chain lookup counts. No throughput
or request-wide allocation reduction has been measured.

Use the original `skills/01-architecture/02-gateway/05-plugin-system.md` live-IP
section and `skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md`,
plus owner-bound IP reference, ConfigData and RealIp regressions. Freeze an exact
IMPL scope after the architecture decision and obtain independent reviews. Existing
baseline tests verify current behavior, not acceptance of a new capability or loan.
