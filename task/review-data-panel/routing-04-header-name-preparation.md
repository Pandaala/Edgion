# Evaluate prepared Route header names

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `786de30c6`.

## Evidence and decision

HTTP and gRPC route `match_header` in their respective
`edgion-gateway/src/routes/{http,grpc}/match_unit.rs` call
`req_header.headers.get(&header_match.name)` for each reached predicate.
HTTP `compile_match_regexes` and gRPC `GrpcMatchInfo::new` already prepare
positionally aligned regexes, but retain string-based name lookup.

The locked http 1.5.0 `header/map.rs` dispatches String lookup through
`HdrName::from_bytes`. In `header/name.rs`, names of 1 through 64 bytes are
mapped into stack scratch and checked for standard names. Longer allowed names
borrow their bytes and map them during hashing and case-insensitive comparison.
This name lookup does not allocate on the request path. Preparing HeaderName
would avoid repeated name parsing; it would not remove an existing per-request
name allocation or eliminate HeaderMap hashing.

Standard HeaderNames use the dependency's standard representation. Custom
HeaderNames own normalized bytes, so simply adding prepared names retains
another name owner alongside the resource String and adds metadata per predicate.
Replacing ownership instead needs a design that preserves original config
spelling and existing resource/runtime boundaries. Compare concrete alternatives
before choosing; no extra global cache, parser, or dependency is presumed.

The decision is whether realistic matching CPU savings justify construction,
snapshot storage and retained-generation costs. Keeping current lookup is a
valid outcome. This is not a correctness defect, a measured regression, or a
general objection to configuration-time preparation.

## Acceptance for later investigation and implementation

- Reuse the canonical http HeaderName implementation. Share any new name
  preparation mechanism across HTTP and gRPC while leaving protocol policies
  explicit. HTTP currently rejects unknown match types; gRPC falls back to Exact.
  Do not silently change that behavior as part of a performance refactor.
- Preserve configured first-equivalent-name semantics, original spelling and
  order, specificity, delegation, AND/OR composition, and regex index alignment.
  See the original
  [HTTP guard](../../skills/04-review/routing/regression-guards/http-header-match-first-name-wins.md)
  and [gRPC guard](../../skills/04-review/routing/regression-guards/grpc-header-match-first-name-wins.md).
- Preserve missing-name failure, incoming repeated-header first-value selection,
  and `to_str().unwrap_or("")` for opaque values. Preserve invalid regex failure
  and absent/empty predicate vectors. Prove lookup parity for standard/custom
  names, mixed case, invalid characters, empty names, the 64-byte boundary,
  maximum allowed length and overlong input.
- Prepare at existing cold construction boundaries and publish with the owning
  immutable snapshot. Check both HTTP builders and gRPC construction, old/new
  readers during updates, and any direct construction or deserialization paths.
  Do not add per-request lazy compilation, locks, logs or metrics.
- Measure hit/miss and early rejection with varied candidate and predicate
  counts. Include standard and custom names, build/update costs, allocation
  count, retained bytes and old generations. Report machine, profile, inputs
  and baseline; distinguish requested bytes from RSS and average from tail latency.
- Obtain independent plan and final review of the selected implementation.
  Run targeted HTTP/gRPC matcher and constructor tests plus repository gates;
  keep unrelated fixture failures visible rather than weakening tests.

No candidate implementation or comparative benchmark has been run in this audit.
