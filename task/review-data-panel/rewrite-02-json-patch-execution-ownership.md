# JSON Patch execution ownership

Status: deferred for user decision. No runtime change.
Source baseline: `140b82587`; locked dependency: json-patch 4.2.0.

## Evidence and scope

ProxyRewrite's compiled plan already owns parsed JSON pointers and literal values.
For each mutating operation, apply_json_operation builds a temporary library
PatchOperation, cloning path/from buffers and obtaining an owned value for
Add/Replace. The library's public patch_unsafe accepts a borrowed operation slice;
its private apply_patches clones Add/Replace values again before insertion.
Literal trees therefore have a gateway staging clone and a library insertion
clone; rendered template strings also receive the latter clone. Empty pointers
and scalar JSON values need not allocate payload buffers, so this is not a
universal per-operation allocation count.

The name patch_unsafe denotes mutation without rollback, not Rust unsafe code.
The gateway intentionally patches an unpublished request-local document and only
publishes after every operation and final serialization pass. Its catch_unwind
boundary maps a library panic to the existing fixed internal failure.

The preceding literal Test borrowing change is complete and separate: Test uses
the gateway's budgeted RFC numeric/structural equality, not the library's test
implementation. The shared profiler representation remains the separate json-01
decision. Neither is permission to rewrite mutation semantics here.

## Decision requested

Choose an ownership design after measuring representative workloads:

- Prepare reusable library operations for static portions of the plan. This can
  remove gateway staging pointer/value copies, but still needs the library's
  insertion clone for reusable values. An additional gateway plan retains duplicate
  metadata across live generations; changing the canonical compiled representation
  instead requires an explicit Resources/Gateway file allow-list and validation review.
- Evaluate a dependency API that borrows pointers and consumes request-owned
  values. Such an API is not exposed by the currently locked implementation;
  dependency maintenance and exact error/operation semantics require review.
- Consider direct mutation only through a separately reviewed canonical owner.
  The dependency's owned-value helpers are private; copying them into the plugin
  casually would create another mutation implementation to maintain.
- Retain the current path if measured savings do not justify ownership or
  maintenance cost. No throughput, peak-memory or allocation benchmark is claimed.

Static and templated operations have different lifetimes. Do not mutate a shared
compiled operation to install a request's template result, add a request-path lock,
or make a reusable static value unavailable to another request.

## Acceptance before implementation

Use IMPL with an exact allow-list and independent plan/final review. Measure
copied bytes and allocation counts for scalar and nested literal values, long
pointers, templates, mixed operation lists and failed operations, alongside retained
metadata for overlapping configuration generations. Separate unavoidable document
ownership from temporary staging copies.

Preserve target validation before template resolution for Add/Replace, source
resolution and profiling before Copy, and value profiling/work charging before
Test path lookup. Keep pointer-length charges, per-operation intermediate document
profile/text/work checks, bounded serialization, fixed failures, and panic isolation.
Removing intermediate checks in favor of a whole-plan library call is not equivalent.

Verify root operations, escaped pointer tokens, object replacement, array insertion
and append, invalid/missing targets, Move into a descendant and same-array index
effects, Copy source ownership, nested values and numeric Test equality. Exercise
templates with missing/present-empty variables, rendering limits and literal inserted
text. Preserve operation ordering, failure with no partial BodyStorage publication,
physical body limits, cancellation behavior and immutable in-flight generations.
Do not endorse a dependency quirk as a requirement without checking the intended
contract; unresolved semantic differences remain decisions, not optimizations.

## Sources and verification limits

- Gateway `plugins/http/proxy_rewrite/plugin.rs`: apply_json_operation,
  resolve_patch_value and modify_json_body.
- Resources `resources/edgion_plugins/plugin_configs/proxy_rewrite.rs`:
  CompiledJsonBodyRewrite, CompiledJsonPatchOperation and compile.
- Cargo.lock and the locked json-patch 4.2.0 source: patch_unsafe, apply_patches,
  private add/replace/move/copy helpers.
- `skills/04-review/http-body/regression-guards/request-body-mutation-physical-cap.md`.
- `skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md`.

This is a source-backed ownership proposal, not validation of an implementation
or evidence of an unbounded memory defect.
