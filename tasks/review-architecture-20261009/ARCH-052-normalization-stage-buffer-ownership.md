---
id: ARCH-052
status: candidate
kind: architecture-debt
priority: P3
partition: PART-197
analyst: /root
independent_verification: confirmed by /root/verify_ip_union
---

# Avoid whole-path copies in unchanged normalization stages

path_normalization::normalize.rs correctly avoids its transformation pipeline
when needs_normalization returns false. Once any transformation is needed,
do_normalize runs four independently allocating helpers under the default
configuration: strip_path_parameters owns a String, percent_decode_whitelist
owns a byte Vec converted into String, merge_slashes owns another String, and
remove_dot_segments owns another byte Vec converted into String. These owners
copy whole intermediate paths even when their individual stage changes nothing.

For /%61dmin, parameter stripping, slash merging and dot removal all preserve
their input. The canonical algorithm returns /admin, but still constructs four
nonempty whole-path buffers. Only the decode stage changes data. Preserving
path parameters skips the first allocation, but does not eliminate the two
unchanged stages after decoding. This is concrete temporary ownership and copy
evidence, not a measured throughput, peak-memory or security regression.

The helper is on actual HTTP/GRPC request entry in
pg_request_filter::apply_path_normalization_entry. Context refresh invokes it
after URI rewrites; PluginSessionAdapter::set_upstream_uri refreshes the current
view and finalizes the forwarding choice. Config path normalization uses the
same implementation for typed route/condition paths. None of these consumers
needs intermediate owned snapshots; they retain only the final current path.
The independently retained raw entry path is an intentional different owner
and must remain outside this optimization.

Keep one canonical four-stage pipeline in its current order, but carry ownership
through stages and leave unchanged stages on the existing buffer or borrowed
input. Acquire output storage only for a stage that changes data; move existing
ownership forward rather than calling to_owned at the final boundary. A private
borrowed-or-owned representation or focused consuming helpers can do this
without adding a public abstraction. An in-place shrinking byte buffer is an
alternative only if its UTF-8/boundary proof and stage semantics are independently
reviewed. No fused new normalization algorithm, global scratch, request cache,
parallel decoder or mock-driven production allocation is needed.

Stage decisions must inspect each preceding stage's output, not just the original
trigger flags: stripping can remove an escape, decoding can introduce slashes
and dots, and merging can expose a dot segment. Preserve strip-before-decode,
one-pass whitelist decoding including backslash, overlapping percent forms,
malformed/reserved/overlong preservation, root clamp and the terminal-dot-only
trailing-slash rule. Preserve parameter opt-out, config/request parity, Exact
versus PathPrefix semantics, frozen entry versus current normalized views,
target authority/query preservation and forwarding/rewrite finalization.
Unchanged whole paths must still return None without pipeline allocations.
Reject-flag validation and its decoding policy are outside this finding.

Original normalization architecture and mandatory review contract require
canonical ordering and view ownership, not separately owned intermediate copies.
The referenced historical design document is absent from the current tree;
current sources/tests and original current architecture are the evidence.
Some architecture prose still says upstream forwarding is always raw and one
write site exists; later sections and current code specify the normalized
default and target-preserving rewrite seam. Record that documentation drift
separately rather than changing the current product contract. ARCH048 concerns
query percent-token allocation with different decoding semantics and scope.

The full 527-line normalization test body was inspected, including ordered
combined transformations, encoded parameters, malformed/overlong/double encodings,
unchanged prefilter cases, overlapping escapes, legitimate versus synthesized
trailing slashes and parameter preservation. Future verification should retain
these and real entry/rewrite/config consumers, add meaningful stage-combination
and allowed UTF-8 cases where missing, and establish that the percent-only witness
does not copy through unchanged stages. Inspect allocation ownership rather than
claiming a benchmark gain without measurement. Keep private access and no new
traffic logs/metrics. Static review only: no builds/tests/runtime experiment or
production edits. Exact source/support scopes belong to REPORT-PART-197.json.
Independent verification is pending.
