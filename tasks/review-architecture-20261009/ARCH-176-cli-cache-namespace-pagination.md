---
id: ARCH-176
status: candidate
kind: behavior-defect
priority: P2
partition: PART-235
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Keep cache namespace filtering in the paginated get path

`edgion-cli/src/cli/commands/get.rs::list_namespaced` sends `--all` directly to `list_all_paginated` before the target-specific branch that applies `filter_by_namespace`. The pagination helper passes the namespace to `EdgionClient::list_namespaced`, but that method explicitly delegates Server and Client targets to `list_all`; the command layer owns their namespace filter. After parsing each response, `list_all_paginated` prints or accumulates its unfiltered `data` array.

The production endpoints confirm that the namespace is not recovered downstream. Controller `api/configserver_handlers.rs::list_resources` accepts only the kind and returns all matching cached resources through `AdminState::cache_list_resources`. Gateway `api/mod.rs::list_resources` likewise calls `list_client_resources` without namespace selection. Both return the list envelope consumed by the CLI. Cache endpoints currently finish in a single page, so a single successful response is sufficient to reproduce the error; no hypothetical pagination server is required.

With cached HTTPRoutes in namespaces `team-a` and `team-b`, `edgion-cli -t client get httproute -n team-a --all -o json` includes both namespaces. Omitting `--all` filters to `team-a`. The same difference exists for the Server target and for table, wide and YAML output. This violates the original CLI overview/command contract: namespace-only cache filtering belongs in the CLI after fetching the full list. It impairs comparison of the three configuration tiers. This is output correctness, not an authentication bypass: the user already has permission to retrieve the cache list.

Apply the existing command-owned namespace filter to each successful Server/Client page before output or accumulation when a namespace was requested. Keep the transport client generic and leave Center's namespaced/key endpoints unchanged. Continue using the server's original continuation token, even when a page has no matching items; filtered emptiness must not terminate pagination. Preserve the existing count adjustment, output formats, limit and continuation requests, error classifications and metadata-only compatibility gate. If filtered empty pages precede the first printed table page, use the streaming printer's existing returned item count to retain the first visible header rather than inventing a new output framework.

A private command-layer page-projection helper shared with the single-page branch is a bounded alternative; adding namespace selection to both cache APIs or embedding CLI policy in EdgionClient is unnecessary. The benefit is one consistent namespace-selection owner across `--all` and single-page output, without additional requests or observability.

Existing `get_test.rs` bodies test `filter_by_namespace` and ordinary Clap inputs, but never wire `--all` with a cache target and namespace. Future verification should cover both cache targets, all four output formats, no matches, Center behavior, and a retained continuation token across an empty filtered page. No tests, builds or requests were executed in this review. No current finding or original review decision was found for this cause; the accepted canonical HTTP-client and admin redaction policies remain intact.
