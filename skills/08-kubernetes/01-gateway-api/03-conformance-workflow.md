---
name: conformance-workflow
description: Cross-session workflow for incrementally driving Gateway API conformance tests, including environment checks, feature analysis, test runs, diagnosis, and recording results
---

# Gateway API Conformance Workflow

A standard workflow for driving Gateway API conformance tests through one item at a time. Every new session can start from here.

## Prerequisites

The OrbStack environment is set up via `setup-orbstack.sh`. If not yet set up:

```bash
cd edgion-tests/conformance
./scripts/setup-orbstack.sh
```

## Workflow steps

### 1. Environment check

```bash
cd edgion-tests/conformance
./scripts/orbstack-dev.sh status
```

Items to check:
- kubectl context = orbstack
- edgion-controller Pod Running
- edgion-gateway Pod Running
- GatewayClass edgion Accepted=True
- Gateway same-namespace has an Address

If anything is wrong, troubleshoot before continuing.

### 2. Choose a test item

The progress checklist is maintained by users / maintainers as needed (no permanent file is kept in the repo).

Priority order:
1. HTTPRoute Core (unchecked items)
2. Gateway Core (unchecked items)
3. HTTPRoute Extended
4. Gateway Extended
5. TLSRoute / GRPCRoute

Users can also specify a particular test directly.

### 3. Feature analysis

Analyze the chosen test. Read:

1. Test source → `edgion-tests/conformance/conformance_test.go` and the upstream `sigs.k8s.io/gateway-api/conformance/tests/<test-name>.go` (grep directly by test name)
2. Known not-implemented items → `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`
3. Controller implementation → `skills/01-architecture/01-controller/`
4. Gateway data plane → `skills/01-architecture/02-gateway/`

Analysis output format:
```
Test: <TestName>
Feature: <FeatureName>
Level: Core/Extended
Expected: PASS / FAIL (needs change) / SKIP (not implemented)
Analysis: <brief description>
Required changes: <if any>
```

### 4. Run the test

```bash
./scripts/orbstack-dev.sh test --run-test <TestName>
```

Or use Go test directly (more flexible regex matching):

```bash
cd edgion-tests/conformance
go test ./... -run "TestConformance/<TestName>" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

### 5. Diagnose failures

If the test fails:

1. **Analyze go test output**: see which assertion failed
2. **Check controller logs**: `./scripts/orbstack-dev.sh logs c`
3. **Check gateway logs**: `./scripts/orbstack-dev.sh logs g`
4. **Check K8s resource state**:
   ```bash
   kubectl -n gateway-conformance-infra get gateway <name> -o yaml
   kubectl -n gateway-conformance-infra get httproute -o yaml
   ```
5. Locate root cause: controller status? Route matching? Data-plane forwarding?
6. **Fails in batch but passes individually?** → Refer to `05-batch-conformance-debugging.md` for three-way log collection and timeline-correlation analysis

### 6. Fix → rebuild → retest

```bash
# After modifying code...

# Rebuild image + rollout (most reliable)
./scripts/orbstack-dev.sh rebuild c    # rebuild controller only
./scripts/orbstack-dev.sh rebuild g    # rebuild gateway only
./scripts/orbstack-dev.sh rebuild      # rebuild both

# Or hotpatch (fastest, cp binary + kill process)
./scripts/orbstack-dev.sh hotpatch c

# Re-run the test
./scripts/orbstack-dev.sh test --run-test <TestName>
```

### 7. Record the result

After the test passes:
1. Tick it off in your own progress checklist (no permanent file is kept in the repo)
2. If there are important findings, update the relevant docs under `skills/08-kubernetes/01-gateway-api/`

### 8. Extended runs (optional)

```bash
./scripts/orbstack-dev.sh test --core                    # Core tests
./scripts/orbstack-dev.sh test --extended                # Extended tests
./scripts/orbstack-dev.sh test --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRouteQueryParamMatching"
```

## Common troubleshooting

| Symptom | Possible cause | Resolution |
|---------|----------------|------------|
| GatewayClass not Accepted | Controller is not started or has crashed | `orbstack-dev.sh logs c` |
| Gateway has no address | Wrong `gateway_address` configuration | Re-run setup-orbstack.sh |
| HTTP/gRPC test timeout | Pod IP unreachable | Use in-cluster mode (recommended) or `--port-forward` |
| gRPC tests hang/crash | port-forward does not support long-lived gRPC connections | Must use in-cluster mode |
| Test SKIP | Feature not declared in supported-features | Check conformance_test.go |
| Pod CrashLoopBackOff | Binary crash | `orbstack-dev.sh logs c/g` |

For the detailed troubleshooting cookbook, see [06-conformance-debug-cookbook.md](06-conformance-debug-cookbook.md).

## Related files

- Test entry point: `edgion-tests/conformance/conformance_test.go`
- Run script: `edgion-tests/conformance/scripts/run.sh`
- Not-implemented items: `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`
