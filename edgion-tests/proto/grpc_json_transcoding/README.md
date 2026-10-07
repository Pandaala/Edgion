# Transcoder descriptor fixture

Regenerate from the repository root:

```bash
protoc -I edgion-tests/proto/grpc_json_transcoding --include_imports \
  --descriptor_set_out=edgion-tests/proto/grpc_json_transcoding/test.pb \
  edgion-tests/proto/grpc_json_transcoding/test.proto
```

The TestService wire fields match the existing test-server service. CodecTest adds
pure codec/WKT coverage without a network implementation. google/api annotations
and HTTP definitions are vendored test inputs from [Google APIs](https://github.com/googleapis/googleapis/tree/master/google/api),
under their embedded Apache 2.0 notices. Generated integration ConfigMap/inline
base64 values must be refreshed together when test.pb changes.
