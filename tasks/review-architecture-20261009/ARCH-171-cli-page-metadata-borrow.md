---
id: ARCH-171
status: candidate
kind: architecture-debt
priority: P3
partition: PART-234
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Borrow the parsed CLI page while projecting metadata rows

print_resource_list_page_streaming in edgion-cli/src/cli/output.rs:207–273 accepts &Value but immediately clones its complete data array at208. The Table and Wide branches then read only each item's name, namespace, kind, creationTimestamp and, for Wide, labels and resourceVersion. The clone recursively copies every unused spec/status subtree before this metadata projection. The pinned serde_json1.0.151 Value derives Clone and owns String, Vec<Value> and Map<String, Value>; this is a full resource-tree copy, not an Arc or a metadata-only snapshot.

The production caller supplies an existing live page. commands/get.rs:227–232 parses the response into data, then passes &data to the formatter for every --all Table/Wide page; the caller retains the same data to read the next continue_token. Ordinary lists receive complete resources, not only keys: Controller namespaced list handlers return center_list_resources, whose AdminState implementation parses each stored entry.content into Value. Thus a table over plugin configurations or other resources with large spec/status bodies briefly retains the parsed page, its complete cloned array and the necessary rendered rows. This remains a short-lived CLI path. No measured latency/RSS reduction, OOM, security leak or request-path regression is claimed.

Borrow the existing array, or a borrowed slice with an empty-slice fallback, for item count and row iteration. Keep the already-owned metadata strings/label projection needed by tabled. print_resource_list in the same file already projects rows by borrowing its input array, so no new abstraction, transport DTO, cache or lifetime framework is needed. The structural improvement removes copy and drop work proportional to the complete page payload while retaining the same public &Value interface and row ownership.

Preserve item order, returned item count, empty/missing/non-array data handling, age boundaries, label ordering and resourceVersion defaults. Keep the first-page header and the current continuation-header/border behavior exactly as implemented. Preserve the Json/Yaml safety branch, ordinary single-resource output, metadata-only opt-in, pagination tokens and termination. The --all Json/Yaml accumulation branch has a separate owned lifetime and is outside this proposal. Do not force --metadata-only to avoid the copy: that changes endpoint/permission semantics and is unavailable on the server/client targets. Do not add a response cap, paging policy, response cache or a global heap-bound claim.

The existing output tests were read in full and protect age boundaries and sorted labels; they do not exercise this page formatter. Actual get tests were read and cover namespace filtering, wrapped-resource extraction and CLI argument routing. Future verification should compare Table/Wide rows, count and first/continuation output for populated and empty/malformed pages with unused nested spec/status fields, while retaining the existing age/label subjects. Source inspection should show that the formatter borrows the caller-owned array through projection rather than cloning complete resource Values. No allocator benchmark or clone-counter framework is required to establish that ownership change.

Original CLI architecture and the debugging three-view contract preserve HTTP transport and resource inspection behavior. The accepted idle-pool decision remains unchanged: canonical client construction, pool2, operator proxy environment, redirects denied and existing deadlines. The original cold-path serialization decision rejects broad trait/API rewrites without demonstrated value; this proposal uses an existing borrowed interface and removes a complete discarded page copy with no cross-crate API change. Exact formatter/source searches found no existing finding for this cause. Static read-only review; no tests, builds, application execution, network, Git operations or writes occurred.
