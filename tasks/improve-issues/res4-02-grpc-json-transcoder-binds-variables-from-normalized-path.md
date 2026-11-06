# res4-02: GrpcJsonTranscoder binds `google.api.http` path variables from the normalised path, violating the HttpRule escaping rules and silently truncating values at `;`

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/grpc_json_transcoder/plugin.rs:101-103` (`bind(session.get_method(), session.get_path(), ...)`)
  - `edgion-resources/src/grpc_transcoding/mod.rs:323-336` (`CompiledGrpcTranscoder::bind`)
  - `edgion-resources/src/grpc_transcoding/mapping.rs:126-143` (`CompiledBinding::bind` + `decode_path(value, multi_segment)`)
  - `edgion-resources/src/grpc_transcoding/mapping.rs:296-346` (`path_pattern`: single-segment `*` -> `[^/]+`; `decode_path` keeps `%2F` only for multi-segment)
  - `edgion-resources/src/path_normalization/normalize.rs:170-215` (BASE: strip `;param`, decode `%2F`, merge, dot removal)

## Summary

The transcoder's binder correctly implements the HttpRule variable-decoding rules
(single-segment `{x}` decodes `%2F` to `/`; multi-segment `{x=**}` keeps `%2F` literally), but it is fed
`session.get_path()`, i.e. the BASE-normalised path. Normalisation has already stripped `;...` from
every segment, decoded `%2F` into a real `/`, merged slashes and removed dot segments. The binder's
careful escaping logic therefore never sees the client's encoding, and three classes of valid REST calls
are broken or silently changed.

## Current behavior (evidence)

- `GrpcJsonTranscoder::run_request` calls `self.compiled.bind(session.get_method(), session.get_path(),
  session.request_query())` (`plugin.rs:101-103`). `get_path()` is the normalised current path (see the
  uri-normalization contract: `ctx.path()`/`session.get_path()` are the normalised view).
- Single-segment variable with an encoded slash (`GET /v1/{name}` with `name="a/b"`, sent as
  `/v1/books/a%2Fb`): normalisation decodes it to `/v1/books/a/b`; the binding regex for `{name}` is
  `[^/]+` (`mapping.rs:307`), so the request no longer matches and returns 404 (`TranscodingError::NotFound`).
  HttpRule explicitly requires clients to percent-encode `/` in single-segment variables precisely so this
  works.
- Multi-segment variable (`{name=projects/*/files/**}`): HttpRule says `%2F` must be preserved in the bound
  value; `decode_path(.., preserve_slash = true)` implements that (`mapping.rs:332-333`), but the input
  already has `/`, so the RPC receives a different resource name.
- Path parameters: with the default `preserve_path_parameters: false`, `/v1/items/foo;rev=2` normalises to
  `/v1/items/foo` (`strip_path_parameters`, `normalize.rs:203-215`); the RPC is invoked with `id="foo"` and
  no error. Resource names containing `;` (allowed by HttpRule and common in GCS-style object names) are
  silently truncated, i.e. the call acts on a different object.
- Dot segments inside a value (`/v1/files/a%2F..%2Fb`) are decoded and removed before binding, again
  changing the bound value rather than rejecting the request.

## Industry comparison

- Envoy's `grpc_json_transcoder` matches the raw `:path` with its own path matcher and applies the HttpRule
  decoding (`url_unescape_spec`: `ALL_CHARACTERS_EXCEPT_RESERVED` by default,
  `ALL_CHARACTERS_EXCEPT_SLASH` for multi-segment); Envoy's `normalize_path`/`merge_slashes` (both
  off by default) never strip `;` or decode `%2F`.
- grpc-gateway likewise binds against the escaped path (`runtime.WithUnescapingMode`) to preserve
  `%2F` semantics.

## Impact

REST clients generated from the same proto (`google.api.http`) behave differently behind Edgion than
behind Envoy/grpc-gateway: resource IDs containing `/` cannot be addressed, multi-segment names are
re-shaped, and IDs containing `;` are silently truncated - an integrity problem (the RPC operates on
the wrong entity) with no error to the caller.

## Suggested direction

- Bind against a transcoder-specific view of the entry path: the raw entry path (`entry_raw_path`) after
  only the steps that are safe for HttpRule semantics (reject - rather than resolve - `.`/`..` segments and
  empty segments; do not strip `;`; do not pre-decode). Route selection and ACLs keep using the normalised
  path, so the security model is unchanged; the binder already owns its own percent-decoding.
- Fail closed when the normalised path and the raw path disagree in a way that would change which binding
  matches (e.g. the raw path contains `..`/`.` segments or `//`), instead of binding the normalised form.
- Document in the plugin guide that `preserve_path_parameters` and `forward-raw-path` do not affect binding.

## Notes

- Checked `skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`
  (bounds, admission, status mapping) and the uri-normalization contract; neither discusses which path view
  the binder consumes or the HttpRule escaping rules. No `tasks/` entry covers it.
- Distinct from `gw-plug3-04` (backend error status) and `gw-plugin2-01` (admission).

## Verification (independent)

- **Verdict**: CONFIRMED (P2)
- `grpc_json_transcoder/plugin.rs:101-103` binds with `session.get_path()`; `session_adapter.rs:1185-1187` returns `ctx.path()`, which is `request_info.normalized_current_path` (`ctx.rs:4047-4049`), not `entry_raw_path`.
- `grpc_transcoding/mapping.rs:306-307` compiles single-segment `*` to `[^/]+`, so a `%2F` already decoded to `/` by BASE normalization (`normalize.rs:258-262`) can no longer match; `decode_path(.., preserve_slash)` (`mapping.rs:315-346`) never sees `%2F` for multi-segment variables.
- `normalize.rs:176-178` strips `;param` by default (`strip_path_parameters`), silently truncating bound IDs containing `;`.
- Distinct from `gw-plug3-04` and `gw-plugin2-01`.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `grpc_json_transcoder/plugin.rs`, `edgion-resources/src/grpc_transcoding/` and `path_normalization/` are unchanged (only `grpc_json_transcoder/budget.rs` and a one-line review-page edit changed); binding still uses `session.get_path()` (`plugin.rs:101-103`).
