# Trusted cold descriptor schema

`descriptor_admission.pb` is the trusted schema for cold wire admission. It contains
standard protobuf descriptor messages and the Google HTTP annotation extension.
It is generated from the existing vendored [Google API definitions](https://github.com/googleapis/googleapis/tree/master/google/api),
whose source files retain their Apache 2.0 notices. Runtime code includes the binary;
it never reads a test fixture, invokes protoc, or fetches a schema.

Regenerate from the repository root with the same installed protoc used for fixtures:

```bash
protoc -I edgion-tests/proto/grpc_json_transcoding --include_imports \
  --descriptor_set_out=edgion-resources/src/grpc_transcoding/descriptor_admission.pb \
  google/api/annotations.proto
```

Encoded custom options other than google.api.http and all uninterpreted options are
unsupported. HTTP payloads share the descriptor depth/node budget. Raw reserved
schema checks prevent untrusted descriptors from shadowing admitted field/extension
shapes before reflection resolves options. Runtime RPC wire admission retains its
existing unknown-field policy.
