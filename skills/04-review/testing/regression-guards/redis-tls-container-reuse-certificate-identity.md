---
name: redis-tls-container-reuse-certificate-identity
description: Use when reviewing integration Redis TLS/mTLS container reuse, generated test certificates, or LinkSys RedisTls handshake failures.
knowledge_type: regression-guards
---

# Redis TLS container reuse follows current-run certificate identity

## Conclusion and scope

Integration Redis TLS and mTLS containers may be reused across work directories
only when the live server certificate still verifies against the host material
that the current run will load into Gateway (`SSL_CERT_FILE` and the generated
`redis-tls-ca` / `redis-tls-client` Secrets). A leftover healthy container is
not reuse evidence by itself. Redis reads TLS files at process start, so a
certificate change must recreate `redis-tls` and `redis-mtls`. Compatible
material still reuses the running containers.

This rule covers the local integration runner dependency lifecycle. It does not
change production Redis TLS verification.

## Rationale and evidence

- `run_integration.sh` prepares certificates and probes Redis before
  `start_all_with_conf.sh` creates a work directory and writes Secrets from the
  same `Services/redis/certs` files.
- `probe_redis_tls_identity` verifies the live listener with the current host
  CA (and client identity for mTLS). It must not treat `docker exec` against
  container-local `/certs` as success: that path can match a leftover process
  after the host files used by Gateway have already changed. On LibreSSL,
  `openssl s_client -verify_return_error` can still exit 0 when the chain does
  not verify; the fail-closed signal is a `Verify return code: 0 (ok)` match
  on that handshake output.
- When the identity probe fails, `docker compose up -d --force-recreate`
  rebuilds only the TLS/mTLS services so Redis reloads the current files.
  Plaintext Redis has no certificate identity and is unchanged.
- `link_sys/redis_assertions.sh` keeps the three live handshakes fail-closed:
  scheme-only default trust, custom CA, and mTLS.

The original defect (`redis-tls-container-reuse-certificate-drift`, 2026-09-15)
reused healthy containers created from an earlier certificate set, then wrote
new work-directory trust material. Ports stayed reachable and Docker reported
healthy, but Gateway recorded `InvalidCertificate(BadSignature)` for
`redis-tls-default-trust`, `redis-tls-custom-ca`, and `redis-mtls`.

## Alternatives and limits

Always recreating TLS containers, as LDAP TLS does, would also prevent stale
in-memory certificates. The runner keeps the fast reuse path when the current
CA still verifies the live server. Do not weaken the LinkSys RedisTls PING
assertions or skip certificate verification to make leftover containers pass.

This does not cover Kubernetes Redis fixtures or production LinkSys TLS
enablement. Those remain in
[shared outbound TLS loading](../../link-sys/regression-guards/shared-outbound-tls-loading.md).

## Verification and re-evaluation

Fresh verification on 2026-09-18:

- `bash cicd/checks/tests/redis_tls_container_reuse_test.sh` passed. The static
  portion requires host-side identity and `--force-recreate`, and it locks the
  LibreSSL fail-closed signal to a `Verify return code: 0 (ok)` match rather
  than the `s_client` exit status. The live portion created three consecutive
  work directories: compatible material reused the same container IDs,
  regenerated material failed the identity probe against leftover containers,
  recreate restored all three handshakes, and generated Secrets matched the
  current host CA and client certificate.

Re-evaluate when Redis TLS probe logic, compose recreate policy, certificate
generation, or the RedisTls Secret output path changes. If the identity probe
flags change, re-check LibreSSL's verify-code output; do not treat
`-verify_return_error` alone as fail-closed.

## Resolution references

- `redis-tls-container-reuse-certificate-drift`: fixed; leftover Redis TLS
  containers cannot be reused after the current run's certificate material
  changes.
