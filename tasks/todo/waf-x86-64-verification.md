# Native WAF verification on x86-64

Status: Pending hardware validation; P2

The image builder already supports linux/amd64. Collect native WAF correctness, resource-bound, and performance evidence on actual x86-64 hardware; this is not an architecture-enablement task.

- Run the supported corpus and relevant parser/matcher tests with the supported Gateway build features.
- Compare Accelerated and Reference execution in release builds on the same host, including clean and adversarial inputs.
- Record CPU, toolchain, features, p50/p95/p99, program memory, scratch, and enforced work bounds.
- Keep ARM timing and threshold claims separate; do not change thresholds without local evidence.

Reuse `edgion-waf/tests/benches/`, the supported CRS fixtures, and `cicd/build-image.sh`.
