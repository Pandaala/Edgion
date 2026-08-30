# AI Backend Policy Integration Dynamic Scenario

`AiBackendPolicyIntegration` creates every scenario-owned resource through the Controller Admin API and removes it during teardown. The directory intentionally contains no manifests, so the same compiled scenario can run against the local filesystem configuration center and a Kubernetes-backed Controller.

Local defaults reuse the existing `ai-gateway` listener on port `31945` and the test server on ports `30001`, `30051`, and `30052`. Kubernetes runners can set:

- `EDGION_TEST_AI_GATEWAY_NAME`
- `EDGION_TEST_AI_PROVIDER_HTTP_BASE`
- `EDGION_TEST_AI_PROVIDER_HTTPS_BASE`
- `EDGION_TEST_AI_PROVIDER_MTLS_BASE`
- `EDGION_TEST_AI_PROVIDER_SNI_BASE`
- `EDGION_TEST_ADMIN_API_URL`
- `EDGION_TEST_GATEWAY_ADMIN_URL`

The runtime-generated `edgion-test/backend-ca` and `edgion-test/backend-client-cert` Secrets are prerequisites. They are owned by the shared integration harness and are never mutated by this scenario.
