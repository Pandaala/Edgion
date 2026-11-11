---
name: system-ca-single-trust-store
description: Use when reviewing what `wellKnownCACertificates: System` (or an omitted CA) trusts on Gateway backend TLS or any Edgion-originated TLS client, or when adding a new outbound TLS consumer.
knowledge_type: regression-guards
---

# `System` CA means one OS bundle file at a fixed path

## Conclusion and scope

`wellKnownCACertificates: System`, or an omitted CA, means the OS CA bundle at the
first existing path of the fixed list `edgion_common::system_trust::SYSTEM_CA_FILES`:

1. `/etc/ssl/certs/ca-certificates.crt` (Debian, Ubuntu, Alpine, shipped images)
2. `/etc/pki/tls/certs/ca-bundle.crt` (RHEL family, the supported Rocky packages)
3. `/etc/ssl/cert.pem` (macOS development hosts)

There is no setting and `SSL_CERT_FILE` does not select it. Selection is strict:
only a missing path moves to the next candidate; anything else at a candidate
path (directory, dangling symlink, unreadable file) is the bundle and fails to
load, so a broken higher-priority bundle is never silently replaced. Custom trust
is a Kubernetes CA reference (`caCertificateRefs` on BackendTLSPolicy, on the
resource, or EGC `outboundTls.validation`), which replaces `System`. Disabled
trust stays `verify: false` where the provider allows it, or plaintext.

In the Gateway the same file backs:

- backend TLS: Pingora `ServerConf.ca_file` is the bundle file. BackendTLSPolicy
  `System`, AI provider HTTPS and dynamic upstream HTTPS peers leave
  `peer.options.ca = None` and use that connector store. `caCertificateRefs` peers
  set `peer.options.ca`; Pingora builds a fresh store and BoringSSL's per-SSL
  verify store replaces the context store, so no default path leaks in;
- every `EffectiveCa::System` consumer, through `system_trust()`: the shared
  `Arc<RootCertStore>` in `link_sys_core/tls.rs` (reqwest clients, Kafka),
  `tls/validation/outbound_client.rs` (OIDC SAN verifier, encrypted health checks,
  all_endpoint_status), `ext_proc/client.rs`, `ldap_auth/plugin.rs`, and the PEM
  re-encoding for tonic (OTLP, etcd `ca_pem_bundles`) and redis-rs
  (`RedisTlsMaterial::root_pem`, a non-optional `Vec<u8>` so redis-rs never falls
  back to its own store).

Startup: Gateway step 4d `install_system_trust()` loads strictly and a BoringSSL
`set_ca_file` precheck (`cli/pingora.rs::validate_backend_ca_file`) runs; failure
refuses startup, and a WARN reports certificates rustls skipped.
`create_server_conf` (after readiness) rechecks BoringSSL loading and that the
file still yields the same certificates, because Pingora loads it again while
building listener connectors and unwraps that load. The Controller loads at
startup step 1c and logs INFO, or WARN when unusable; its System consumers (etcd
conf-center TLS without `ca_cert_file`, the OAuth2 credential source, the HTTPDNS
background resolver) then fail closed. `edgion-cli` loads it with its first client
and prints a warning for an `https://` server when it is unavailable.

Loading rules: regular files only (checked before open, so a FIFO cannot block),
reads bounded to 8 MiB, any malformed PEM section rejects the file, CERTIFICATE
entries rustls rejects and `TRUSTED CERTIFICATE`/`X509 CERTIFICATE` sections are
skipped and counted, and a file with no usable certificate is rejected.

## Rationale and evidence

Before infra3-03, backend `System` used BoringSSL default verify paths while all
rustls clients used compiled `webpki_roots::TLS_SERVER_ROOTS` (and tonic/redis-rs
webpki features). A private root added to the image trusted backends but not
OIDC, LDAP, ext_proc, OTLP or LinkSys, and root updates followed two sources.
Gateway API defines `System` as the implementation's system trust store. Envoy
Gateway translates it to the fixed `trusted_ca` file
`/etc/ssl/certs/ca-certificates.crt`, and Envoy itself loads only the configured
`trusted_ca`. Edgion follows that model with a short fixed list because it also
ships RHEL-family packages. The etcd provider previously added no roots for
`System` (tonic 0.14 loads none implicitly), and etcd-client gives `https://`
endpoints an empty tonic config when no TLS block is set; both now receive the
bundle (`default_tls_options` applies only to all-`https://` endpoint sets,
because etcd-client rejects TLS options for `http://` and applies them to
scheme-less endpoints).

Call chain (checked in review): `ServerConf.ca_file` →
`ProxyServiceBuilder::new(&server_conf, ..)` → `ConnectorOptions::from_server_conf`
→ h1/h2 `TransportConnector` → `tls::Connector::new` → `set_ca_file(..).unwrap()`.
TCP/TLS-route connectors (`TransportConnector::new(None)`) only dial plain TCP.

## Alternatives and limits

- **BoringSSL default locations remain in the backend store.** boring 5.4
  `SslConnector::builder` calls `set_default_verify_paths()` before Pingora adds
  `ca_file`. Backend trust is therefore the bundle plus BoringSSL's default file
  and hash directory: `SSL_CERT_FILE`/`SSL_CERT_DIR` when set (they replace, not
  extend, the defaults), otherwise `/etc/ssl/cert.pem` and `/etc/ssl/certs`; the
  directory is consulted on every verification. Pingora's
  `TransportConnector::new(None)` (TCP, TLS-terminate and passthrough listeners)
  runs openssl-probe's `init_ssl_cert_env_vars`, which may set those variables
  inside the process, so the result can depend on listener construction order.
  Accepted: in the shipped Debian images the probe, the hash directory and the
  bundle come from the same `update-ca-certificates` set; on RHEL-family hosts
  `/etc/ssl/cert.pem` and `/etc/ssl/certs` link into `/etc/pki/tls`, which
  `update-ca-trust` fills from the same extracted set as the bundle. Divergence needs
  certificates dropped into the hash directory without regenerating the bundle, a
  different OS layout, or an operator-set `SSL_CERT_*`. Rejected for now: a
  Pingora-fork change giving `Connector::new` an empty store when `ca_file` is set
  (needs a fork release), and per-peer `peer.options.ca` for System peers
  (rebuilds a ~140-certificate store per new upstream connection). Revisit if a
  `System` override is ever added, a non-Debian image is shipped, or images stop
  keeping the hash directory and bundle in sync.
- **Rejected: a `system_ca_file` setting and environment-driven probing
  (`SSL_CERT_FILE`, openssl-probe).** Kubernetes CA references already express
  custom trust; a fixed list keeps trust predictable and free of environment
  influence. Cost: a base image whose bundle is at none of the listed paths must
  link or mount it at one, because the Gateway refuses to start without it.
- **Rejected: falling back to compiled webpki roots or starting with an empty set.**
  Either would recreate two meanings.
- **Trust differs from the former webpki set.** The OS bundle can contain roots
  Mozilla distrusts by date (webpki-roots 1.0.11 omits them, for example Entrust)
  and cannot carry Mozilla's external name constraints (webpki-roots limits the
  TUBITAK root to `.tr`; `RootCertStore::add` keeps only in-certificate
  constraints). Accepted: backends already trusted the OS bundle, and Envoy, Go
  and curl behave the same; the distribution's `ca-certificates` package owns
  distrust. Recorded in the 2026-11-10 release note.
- **rustls and BoringSSL read the same file differently.** rustls skips entries it
  cannot use (counted and WARNed at Gateway startup) and `TRUSTED CERTIFICATE`
  sections, which BoringSSL loads with their trust settings. rustls accepts any
  certificate as an anchor without date checks, while BoringSSL needs a self-signed
  root and checks its validity. The ring provider cannot verify P-521 signatures.
  Distribution bundles contain self-signed CERTIFICATE roots, so these differences
  do not arise for the shipped images.
- **Exempt clients that load roots themselves** are not `wellKnownCACertificates`
  consumers: Controller ACME (instant-acme `try_with_platform_verifier`, or
  `builder_with_root` for a custom CA) and kube-client (kubeconfig CA or platform
  verifier). Do not add a new `System` consumer that loads its own roots
  (`webpki_roots`, `with_webpki_roots`, `with_native_roots`, `rustls-native-certs`);
  use `system_trust()`.
- Clients share one `Arc<RootCertStore>`; per-request client builders
  (all_endpoint_status) pay no anchor copy. OTLP, etcd and Redis re-parse the
  shared PEM per client or pooled connection (cold path).

## Verification and re-evaluation

- Run 2026-11-10, passing: `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets` (no warnings in changed files),
  `cargo test -p edgion-common --lib`, targeted `cargo test -p edgion-gateway --lib`
  (cli, tls::validation, ext_proc, ldap_auth, otlp, redis, kafka, health,
  openid_connect, all_endpoint_status), `cargo test -p edgion-resources --test
  crd_coverage`, `validate_agent_docs.py`, `check_ssa_force.py`,
  `check_metrics_inventory.py`, the Gateway API transform contract check.
- Integration run 2026-11-11 on macOS (bundle `/etc/ssl/cert.pem`), passing: the
  main suite (`run_integration.sh`, 286 passed, 0 failed), every standalone job of
  `run_all_integration.sh`, and with `--full-test` `EdgionPlugins/LdapAuthTls`,
  `EdgionPlugins/AllEndpointStatus`, `EdgionPlugins/ExtProc` (including
  `Dyn_ExtProcTlsPolicyRecovery`) and the `-r Gateway` sweep (including
  `Dyn_EncryptedActiveHealthChecks`, `Dyn_AllEndpointStatusTls`,
  `Dyn_LinkSysDisabledTls`).
- Covering tests: `edgion-common/tests/unit/system_trust_test.rs` (shared root
  store, PEM round trip, skipped entries excluded from `pem()`, rejection of
  empty/malformed/missing/non-regular/oversized files, strict fixed-order
  selection including a broken higher-priority path, empty fail-closed set);
  `tls_test.rs::verified_http_roots_are_the_process_system_trust_and_alpn_is_explicit`;
  `providers/etcd/config_mapping_test.rs::system_trust_is_passed_explicitly_and_custom_ca_replaces_it`
  and `omitted_tls_gives_all_https_endpoints_system_trust_only`;
  Redis material assertions in `redis_test.rs` and `tls_test.rs`; Gateway
  `server_conf_tests.rs` (`ServerConf.ca_file` equals the System bundle file,
  BoringSSL precheck). SAN-verifier, LDAP and OIDC tests use generated CAs.
  `all_endpoint_status/transport_test.rs::gateway_client_pairing_and_probe_capture_share_checked_publication`
  still uses `System` and needs a host CA bundle (CI and development hosts have one).
- Accepted gaps, not tracked as tasks: no integration scenario exercises backend
  and outbound `System` together; the Gateway startup refusal and the
  `create_server_conf` changed-file refusal are inline and not unit-tested; the
  standalone Rocky 9 lifecycle test was not re-run for this change.
- Re-evaluate on a Pingora/boring upgrade that changes `SslConnector::builder` or
  `ConnectorOptions`, if a `System` override is requested, or if another OS family
  or base image is supported.

## Resolution references

- infra3-03 — `wellKnownCACertificates: System` resolved to two trust stores in the
  same Gateway; confirmed and re-verified on feature-10-28, fixed 2026-11-10 by
  unifying both paths on the OS bundle at a fixed path. A multi-agent deep review
  the same day found the BoringSSL default-path union, unbounded reads, probing
  edge cases and the RHEL-family package layout; the design moved to a fixed path
  list without a setting.
