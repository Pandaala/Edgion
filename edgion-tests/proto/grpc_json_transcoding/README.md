# Test service schema and transcoder descriptor fixture

`test.proto` owns the `test` package used by both the checked-in Rust stubs and
the transcoder descriptor. To regenerate `edgion-tests/src/proto_gen/test.rs`, run
from the repository root:

```bash
./edgion-tests/integration/scripts/proto/generate_test_proto.sh
```

The wrapper also works from another directory when invoked by its absolute path.
Alternatively, run `cargo run -p edgion-tests --bin build_test_proto` from the
workspace; the generator resolves paths from its compile-time crate directory.
Install `protoc` (with its standard protobuf includes) and the Rust `rustfmt`
component first. Generation uses the workspace tonic/prost versions and formats
the result with the workspace `rustfmt.toml`. It publishes `test.rs` only after
code generation and formatting succeed; missing input or tool failure returns
an error and preserves the previous artifact. Review the generated diff and
compile both `test_client` and `test_server` before committing it.

The complete package includes CodecTest/OverlapTest stubs and codec/WKT types;
those fixture-only services are not registered by the test server. Imported
Google API annotation Rust output is temporary because the test package does
not reference it. Ordinary integration preparation uses the checked-in Rust
file and does not run this command. Common's separate build-time protobuf
generation is unchanged.

## Descriptor refresh

Rust stub regeneration does not rewrite `test.pb` or embedded descriptors. When
changing the schema, review whether those fixtures also need regeneration.

Regenerate from the repository root:

```bash
protoc -I edgion-tests/proto/grpc_json_transcoding --include_imports \
  --descriptor_set_out=edgion-tests/proto/grpc_json_transcoding/test.pb \
  edgion-tests/proto/grpc_json_transcoding/test.proto
```

The TestService wire fields match the existing test-server service. CodecTest adds
pure codec/WKT coverage without a network implementation. OverlapTest supplies
deliberately intersecting HTTP annotations for publication rejection tests.
google/api annotations
and HTTP definitions are vendored test inputs from [Google APIs](https://github.com/googleapis/googleapis/tree/master/google/api),
under their embedded Apache 2.0 notices. Generated integration ConfigMap/inline
base64 values must be refreshed together when test.pb changes.
