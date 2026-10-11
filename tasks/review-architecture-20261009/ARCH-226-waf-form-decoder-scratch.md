---
id: ARCH-226
status: candidate
kind: architecture-debt
priority: P3
partition: PART-161
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# ARCH-226: Borrow unchanged form fields before publishing owned WAF body entries

## Problem and current ownership

`edgion-waf/src/seclang/body.rs:569–578` decodes every URL-encoded name and value into a temporary `Vec<u8>`. Its private `percent_decode` at 813–847 reserves the input length and copies every byte even when the field contains neither `+` nor `%`. `BodyBuilder::push` at 523–538 then fallibly copies the decoded slices into independently owned `BodyEntry` names and values. The temporary decoder vectors are dropped after that segment's ARGS and ARGS_NAMES entries have been created.

For a plain nonempty segment such as `item=from-body`, this creates one disposable payload allocation and copy for the name and another for the value before the required publication copies. Empty fields do not imply a heap allocation and are excluded from that allocation count. Final entry ownership is legitimate: published body entries outlive the local decoding step. The unnecessary ownership is only the unchanged intermediate decoding result.

This is a real request consumer rather than an unused helper. The Gateway WAF Full path in `edgion-gateway/src/plugins/http/waf/mod.rs:564–658` obtains the canonical capped complete request body and publishes a nonstructured body through `Transaction::publish_request_body_with_json_limits`. `json_inspection_runtime.rs:137–167` delegates a non-JSON/non-XML processor to `Transaction::publish_non_json_body`; `transaction.rs:1491–1585` builds and admits the resulting materialized publication. The existing transaction test at `transaction_test.rs:1488–1527` publishes the plain `item=from-body` form with automatic content-type selection and observes body ARGS and request-body semantics.

## Bounded improvement

Keep the change inside the private form decoder and its immediate caller. Return a borrowed-or-owned byte result, for example `Cow<'a, [u8]>`: if the input contains neither `+` nor `%`, return the original slice; otherwise run the existing fallibly reserved decoder into an owned vector. `parse_urlencoded` continues to pass slices into the unchanged owning `BodyBuilder::push`. No borrowed field enters `BodyEntry` or survives publication, and no public API or generation representation changes.

The concrete benefit is removal of disposable allocations and payload copies for unchanged nonempty form fields on the request path. This is not a measured throughput, RSS, or total-request heap improvement. The marker preflight adds inspection work for encoded fields; implementation assessment should account for that tradeoff rather than claiming all inputs become faster.

An alternative is to special-case plain names and values in `parse_urlencoded`, but that repeats the decoder's ownership choice at both call sites. Transferring decoder vectors directly into publication would require a wider redesign of duplicate name/value ownership and admission order. A private borrowed intermediate is the narrower boundary.

## Required invariants

Preserve the tolerant byte semantics exactly: `+` becomes space; valid percent escapes decode once; malformed or truncated escapes remain literal; hex case is accepted; `%2B` remains distinct from `+`; arbitrary binary bytes and NUL are permitted. Do not substitute a strict UTF-8, URI, or string decoder. Split on `&` before decoding, skip empty segments, split only the first `=`, and preserve empty values, duplicate fields, ordering, and separate ARGS/ARGS_NAMES publication.

Keep the current segment work charge before decoding and every `BodyAccounting::push` charge, count, name bound, decoded-byte bound, combined-size calculation, error order, descriptor admission, and required final fallible copy. The no-transform path needs no intermediate allocation; the owned decoding and final publication paths must still report allocation failure. Removing an unnecessary allocation is not a promise to preserve an allocation failure that could only originate from that removed allocation.

Preserve body processor selection, Full/prefix policy, DetectionOnly malformed-body behavior, transaction phase ownership and aggregate publication admission. Canonical request BodyStorage remains the transport owner. This creates no additional body cache, no response aggregation, no borrowing from BodyStorage into retained entries, and no change to JSON, XML, multipart, capture, transformation, or scheduling owners. Add no logging, metrics, runtime dependencies, or shared decoding framework.

## Original decisions and validation

The original native WAF body and fused-inspection contracts deliberately distinguish logical publication accounting from physical retention and preserve bounded owned publication. Their accepted borrowed transformation and selected-result work does not remove this separate form-decoder intermediate. The multipart boundary guard concerns bounded delimiter search; its parser ownership is outside this proposal. The WAF optimization specification's SIMD/DFA/shared-scheduling adoption gates do not authorize those unrelated changes here.

`body_test.rs` was read completely, including duplicate/empty form fields, malformed `%Q0`, and exact/one-less logical publication limits. The selected production transaction test and Full Gateway consumer were also read. Implementation verification should cover borrowed identity for plain byte fields, owned results for `+`/`%`, valid and malformed/truncated escapes, binary/NUL bytes, split/order/duplicate behavior, and unchanged exact/one-less logical work/count/decoded-byte errors through publication. Existing final-entry ownership must remain demonstrable after the local input goes out of scope.

This differs from the reviewed URI/strict transcoder and DSL XML scratch findings: this is the private tolerant WAF form-byte decoder and its own publication seam. No implementation, tests, benchmark, external oracle, or live traffic was executed for this read-only review. Current source establishes the redundant intermediate ownership; it does not establish a production incident or a numerical performance benefit.
