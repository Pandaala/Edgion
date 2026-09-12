---
name: bounded-body-text-reads
description: Use when reviewing DSL Body read allocation bounds, lossy UTF-8 range decoding, scan accounting, or body runtime ownership.
knowledge_type: regression-guards
---

# DSL Body text reads are bounded during decoding

## Conclusion and scope

`Body.read(offset, max_bytes)` and `Body.read_all()` use one canonical
`RequestBodyState::read_text_range` operation over the selected raw snapshot
range. `LossyTextScanner` feeds a bounded collector without first collecting
the full raw range. Logical output and requested output capacity are limited
before each append/reservation, including invalid UTF-8 expansion and EOF carry.

## Rationale and evidence

The original `bound-dsl-body-reads-before-allocation` finding identified a raw
range allocation followed by complete lossy conversion before the VM string
check. `read_all()` additionally had a raw-byte precheck that could not bound
replacement-character expansion. The correction keeps that precheck but uses
the bounded collector for both opcodes.

- Generic collection and UTF-8 adaptation:
  `edgion-gateway/src/request_body/operation.rs`.
- Range validation, scan preflight, immutable snapshots, and fetched-byte
  accounting: `edgion-gateway/src/request_body/storage.rs` and `state.rs`.
- DSL error conversion and session-owned mutations:
  `edgion-gateway/src/plugins/http/dsl/lang/body_ops.rs`.
- One scan account shared with opaque HTTP uploads:
  `edgion-gateway/src/plugins/http/dsl/lang/body_scan_budget.rs` and `builtins.rs`.
- DSL-specific JSON/regex scanners belong in `lang/body_scanners.rs`, not the
  generic request-body core.

`OutputLimitExceeded` is a scan outcome retaining `bytes_scanned`, not a
`BodyOperationError`. The facade settles that accounting before raising
`StringTooLong`, whose length is a lower bound (`limit + 1`) on early overflow.
Raw-length preflight retains its known-length diagnostic. Storage/allocation
failures remain `BodyAccess` and fixed fail-close; output-limit errors retain
the configured DSL error policy. No body bytes or storage details are logged.

## Alternatives and limits

Do not use a `push` error for output overflow: it loses the completed fetch
accounting in the existing scanner protocol. Do not use ordinary geometric
`String` growth without a capped capacity target. Allocation failure is not
evidence of output overflow.

The bound covers requested text capacity and retained text, not allocator
rounding, fixed raw scan scratch, bounded snapshot metadata, or the temporary
`String` to `Arc<str>` conversion. Different source segments can change how
many bytes are fetched before early stop; all fetched bytes must be charged.
An error without accounting retains the existing conservative reservation.

`Value::Body` remains a cheap host-neutral tag with live-generation semantics.
Do not insert a storage pointer or cursor into it. Each operation fixes its
own immutable snapshot; explicit transfer readers remain frozen. Mutation
must pass through `PluginSession` so phase, replay owner, and transactional
generation checks remain effective. Main upstream responses stay streaming.

## Verification and re-evaluation

Focused tests cover the standard lossy decoder at every split and range,
memory/spill/mutation segments, exact/zero/overflow limits, capacity targets,
raw scan preflight, early-stop charges/refunds, live aliases, frozen readers,
late-read/mutation boundaries, and output-limit versus body-failure policies.
Fresh commands and independent final-review results are recorded in
`tasks/todo/dsl-body-capability-runtime/02-implementation.md`.

Re-evaluate when scanner accounting, range slicing, decoder carry, capacity
growth, VM error policy, body lifetime, or upload cancellation changes. Keep
the 24-byte `Value` invariant and the single canonical request-body cache.

## Resolution references

- `bound-dsl-body-reads-before-allocation` — implementation task:
  `tasks/todo/dsl-body-capability-runtime/dsl-body-capability-runtime.md`.
- Canonical architecture: `skills/01-architecture/02-gateway/dsl-engine.md`
  and `dsl-engine-security-model.md`.
