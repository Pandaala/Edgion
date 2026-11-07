---
name: multipart-boundary-search-bounded
description: Review native WAF multipart boundary discovery, its worst-case work, and parsing semantics.
knowledge_type: regression-guards
---

# Multipart boundary discovery stays linear

## Conclusion and scope

The full request-body multipart parser in `edgion-waf` finds each boundary in
linear total time, including attacker-controlled near matches. It preserves
the existing recognition rule: `--boundary` at byte zero or after CRLF, then
either CRLF or `--`. It preserves the existing part and header extraction and
the body byte, decoded byte, part, header, and work limits. The parser does
not log body content.

`BodyLimits.max_work_bytes` is a bounded work model, not an exact CPU
instruction count. Linear discovery satisfies the work-bound requirement
without changing the existing charge order or making ordinary multipart
requests fail under a lower effective budget.

## Rationale and evidence

The previous `find_multipart_boundary` repeatedly called `find_bytes`, which
compared every input window against the entire delimiter. Its worst-case
comparison work was proportional to body length times boundary length and
was not charged to `BodyBuilder.work_bytes`. The full-inspection Gateway WAF
path publishes a complete body before evaluating Phase 2 rules, so this work
ran synchronously on a request task.

The current parser builds bounded delimiter bytes on the stack. For boundary
values without CR or LF, a reusable `memchr::memmem::Finder` scans for
`CRLF--boundary`; the start-of-body candidate is checked separately. The
finder's iterator has a documented linear worst-case search bound and keeps
prefilter state across rejected candidates. Complete line-delimiter matches
cannot overlap when the boundary contains no CR/LF. A boundary containing
CR/LF takes a KMP path with one bounded prefix table, retaining the old
candidate ordering even for overlapping complete matches. The cursor is the
lowest permitted `--boundary` start; the line search includes a two-byte
lookback for its preceding CRLF. Between parts the search position advances;
rechecking an accepted delimiter costs at most its length, and each part
consumes at least that delimiter. Total discovery work remains linear in the
body and boundary lengths.

The source is `edgion-waf/src/seclang/body.rs`. Tests in
`edgion-waf/tests/unit/seclang/body_test.rs` compare boundary positions with
the old search on every cursor for varied boundaries, including an overlapping
CR/LF example. Deterministic high-bit pseudorandom samples also cover repeated
bytes without the low-bit alternation of the original fixture. The tests
exercise long near matches, normal fields and files, malformed input, and
limits. The transaction test checks Phase 2 results in
both enforcement and detection-only modes.

### Boundary-adjacent delimiter

The CRLF stripped before the next delimiter can be the same CRLF that ended
the previous boundary line, as in `--edge\r\n--edge--`. Subtracting it then
put the part end before the part start, and the direct slice panicked on the
request path (F-S5-1). The parser now treats `next_boundary < part_start` as
`BodyError::Malformed`, strips the CRLF only when the resulting end is not
before the start, and takes the part with `input.get(..)`, so a future
offset error yields `Malformed` rather than a panic. The adjacent case is
therefore an empty part, which is malformed like any part without a header
separator. `multipart_boundary_directly_after_boundary_is_malformed_not_panic`
pins the former panic and `multipart_zero_length_part_stays_malformed` pins
the unchanged result for `--edge\r\n\r\n--edge--`.

## Alternatives and limits

Coraza delegates multipart parsing to Go's `mime/multipart.Reader`; replacing
Edgion's parser with a general MIME parser would change its accepted syntax
and collection behavior. A single KMP path and a KMP path with `memchr` byte
skipping also have linear bounds, but were slower on near-match inputs in the
local comparison. The optimized finder was slightly slower than byte-skipping
KMP on plain data. The chosen split favors the much larger difference on
attacker-controlled content while retaining a bounded exceptional path.

The performance measurements are same-host observations, not a guarantee
about every CPU or request distribution. The boundary remains limited to 200
bytes; request-body capture and other body processors have separate policies.

## Verification and re-evaluation

During this fix, `cargo test -p edgion-waf --lib multipart` passed five tests,
and `cargo test -p edgion-waf --lib` passed 394 tests with two pre-existing
ignored cases. Workspace check, Clippy, formatting, agent-doc validation, SSA,
metrics inventory, Pingora source policy, Gateway API transform, and unit-test
layout checks passed. The full workspace test suite was not run.
An optimized same-host manual benchmark during candidate selection, before the
final stack-buffer cleanup, used five samples per case and compared
the old search, pure KMP, byte-skipping KMP, and the finder including searcher
construction. For 1 MiB of near matches, the finder median was 5-10 us for
70- and 200-byte boundaries, versus 1.5 ms for the old search and 0.8-0.9 ms
for byte-skipping KMP. For 1 MiB of exact delimiters with invalid suffixes,
the finder median was about 0.44 ms versus about 1.45 ms for the old search.
On 8 MiB of near matches, finder medians were 32-93 us versus 12 ms for the
old search. Full multipart publication on near-match bodies measured 19 us
at about 1 MiB, 570 us at about 8 MiB, and 1.14 ms at about 15 MiB. The
manual benchmark source was removed after measurement; these are elapsed-time
medians from this host, not persistent CI gates or exact CPU measurements.

The current reproducible benchmark is
`cargo bench -p edgion-waf --bench multipart_boundary`; set
`EDGION_WAF_BENCH_SMOKE=1` for a short run. It measures the public
`Transaction::publish_request_body` call after transaction construction and
header evaluation, then verifies the Phase 2 result outside the timed region.
It reports CSV p50/p95 for 70- and 200-byte boundaries; plain file content,
near matches, and exact delimiters with invalid suffixes; and body sizes from
64 KiB through 8 MiB, plus a 15 MiB near-match case. A 101-sample optimized
run on aarch64 macOS during this follow-up measured 1 MiB near-match p50 at
about 0.45-0.51 ms and 15 MiB near-match p50 at about 8.9-9.2 ms. This
end-to-end publication benchmark includes parsing and allocation, so its
numbers must not be compared directly with the earlier search-only timings.

Re-evaluate if the accepted boundary syntax, multipart parse flow, work-budget
model, finder dependency, or supported CPU targets change. Repeat the
same-host comparison before replacing the search strategy.

## Resolution references

- `waf-multipart-boundary-work-budget`: fixed the unbounded delimiter search
  and retained overlapping-candidate semantics.
