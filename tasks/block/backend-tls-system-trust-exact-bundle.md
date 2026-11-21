# Make backend `System` trust exactly the configured OS bundle

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-11-12 |
| Status | blocked |
| Resume condition | The maintainer has deferred the decision on whether to change backend trust. Resume implementation only if the maintainer chooses the proposed change. |
| Type | bugfix |
| Priority | P3 |
| Issue | N/A |
| Source | [system-ca-single-trust-store](../../skills/04-review/tls/regression-guards/system-ca-single-trust-store.md) |

## AI guide summary

Gateway backend TLS for `wellKnownCACertificates: System` (or an omitted CA)
should trust only the fixed OS bundle file that every other `System` consumer
uses. Today BoringSSL default verify paths are added to the same store. The fix
belongs in the Edgion Pingora fork, followed by a lockfile advance.

## Evidence and scope

- boring 5.4 `SslConnector::builder` calls `set_default_verify_paths()` before
  Pingora adds `ServerConf.ca_file`. Backend `System` trust is therefore the
  bundle plus BoringSSL's default file and hash directory: `SSL_CERT_FILE` /
  `SSL_CERT_DIR` when set, otherwise `/etc/ssl/cert.pem` and `/etc/ssl/certs`.
- `TransportConnector::new(None)` builds `tls::Connector::new(None)`, whose private
  `init_ssl_cert_env_vars` wrapper runs openssl-probe and may set those variables
  inside the process, so the effective store can depend on listener construction
  order.
- The risk is accepted until this lands; the guard records why the shipped Debian
  and RHEL-family layouts keep the extra paths in sync with the bundle, and when to
  raise this task's priority (a non-Debian image ships, or images stop keeping the
  hash directory and bundle in sync).
- Fix: in the Edgion Pingora fork (`Pandaala/pingora`), make a configured `ca_file`
  replace the default store in `Connector::new`. Because the default paths come
  from boring's `SslConnector::builder`, calling `set_ca_file` alone cannot remove
  them; install a fresh store loaded only from `ca_file` (for example
  `set_cert_store_builder(X509StoreBuilder::new().unwrap())` before `set_ca_file`).
  Building the context without `SslConnector::builder` is not an option without
  replacing the `SslConnector` hostname-verification setup. Land it on the fork's development branch, then
  advance `Cargo.lock` as described in
  [the Pingora fork branch-lock policy](../../skills/04-review/supply-chain/mandatory-design/pingora-fork-branch-lock-policy.md).
- Out of scope, already rejected in the guard: per-peer `peer.options.ca` for
  `System` peers, a `system_ca_file` setting, and environment-driven probing.

## Acceptance

- [ ] Fork connector test in a dedicated integration target (a new
      `pingora-core/tests/<name>.rs` containing only this test, built with the
      `boringssl` feature, so it runs in its own process): with `ca_file` naming one
      bundle, a TLS handshake to a server whose CA is reachable only through
      `SSL_CERT_FILE` / `SSL_CERT_DIR` (stand-ins for the default file and hash
      directory) fails, and a handshake to a server signed by the bundle succeeds.
- [ ] `Cargo.lock` resolves the whole fork family to one commit that contains the
      change, `cicd/checks/check_pingora_source_policy.py` passes, and the Gateway
      build and tests required by the branch-lock policy's advance procedure pass.
- [ ] Update the guard's "Alternatives and limits" entry from accepted risk to
      resolved, keeping the history.
