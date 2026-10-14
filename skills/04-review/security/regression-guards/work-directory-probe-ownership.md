---
name: work-directory-probe-ownership
description: Review startup WorkDir writability checks for exclusive creation and cleanup ownership.
knowledge_type: regression-guards
---

# Work-directory probe ownership

## Conclusion and scope

`WorkDir::validate` is the shared Controller/Gateway startup writability check.
Its private `WriteProbe` creates a candidate exclusively with `create_new`, writes
through the returned handle, closes it, and attempts removal on both write success
and failure. Only creation collisions are retried, with at most 16 attempts.
Creation/write errors fail validation; cleanup errors retain the existing warning
policy and do not replace the write result. No cleanup follows failed creation.

## Rationale and evidence

ARCH-049 identified that the former fixed `.edgion_write_test` path was written
with `std::fs::write`, which truncated an existing file or symlink target, then
unlinked without ownership. The base-directory canonicalization did not protect
this child. This was a conditional code witness, not a reported incident.

The owner is [work_dir.rs](../../../../edgion-resources/src/work_dir.rs); both
[Controller](../../../../edgion-controller/src/cli/mod.rs) and
[Gateway](../../../../edgion-gateway/src/cli/mod.rs) validate before logging setup.
Directory layout, precedence, public API and logging bootstrap stay unchanged.
The [graded rules](../../../00-design-rules.md) require minimal changes,
canonical ownership, and binary-owned logging; this correction adds no traffic
observability or dependencies.

## Alternatives and limits

PID/counter/attempt names reduce collisions; exclusive creation provides the
safety guarantee. A fixed exclusive name would unnecessarily reject writable
directories with an occupied name. Permission bits cannot establish actual
writability, and an existence check followed by creation races.

Protection covers objects present when each candidate is created. It does not
protect against a hostile directory writer replacing the pathname afterward.
Process termination may leave a probe behind; future startup skips collisions.
A cleanup warning may be invisible before the binary installs its subscriber;
changing logging startup is outside this correction.

## Verification and re-evaluation

[Unit tests](../../../../edgion-resources/tests/unit/work_dir_test.rs) cover
legacy files and Unix links, candidate file/link collisions, exhaustion,
non-collision errors, cleanup after successful and failed writes, and Unix
best-effort cleanup failure. Write failure uses a read-only handle to the owned
file without production injection hooks. Existing layout and readonly tests
remain; the readonly test is privilege-sensitive.

Fresh verification: `cargo test -p edgion-resources --lib work_dir::tests` passed
all 17 tests, including the existing readonly-directory case.
`cargo fmt --all -- --check`, `python3 cicd/checks/validate_agent_docs.py`, and
`python3 cicd/checks/check_unit_test_layout.py` passed.
`cargo check --workspace --all-targets -j1` and
`cargo clippy --workspace --all-targets -j1` passed; Clippy reported eight distinct
warnings in unrelated test files and none in this correction. Independent final
review passed.

Re-evaluate if probe creation, write, cleanup, retry policy, startup consumers, or
the directory threat model changes. The correction makes no durability or
hostile-unlink guarantee.

## Resolution references

- ARCH-049: exclusive work-directory startup probe; implementation and independent final review verified.
