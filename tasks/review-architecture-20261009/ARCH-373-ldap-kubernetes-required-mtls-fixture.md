---
id: ARCH-373
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-500
analyst: /root/analyze_conversion
baseline: 8f1c53149be48bb3de89f8cf43a7f08d7ecff602717ec079f6b7ee316705c306
---

# Make the LDAP TLS Kubernetes mTLS case reach a certificate-required server

The registered `mtls_client_cert_bind_succeeds` case in `edgion-tests/src/client/suites/edgion_plugins/ldap_auth_tls/ldap_auth_tls.rs:101-139` describes a server that requires a client certificate and passes on HTTP 200. Its Kubernetes fixture does not establish that prerequisite. `k8s/conf/EdgionPlugins/LdapAuthTls/11_Deployment_edgion-test_openldap-tls.yaml:63-66` configures the single OpenLDAP server with `LDAP_TLS_VERIFY_CLIENT: "try"`; `12_Service_edgion-test_openldap-tls.yaml:10-17` directs both the CA-only port 33636 and the purported mTLS port 33637 to that same server's port 636. The mTLS plugin supplies a client identity, but its positive bind result cannot distinguish required identity presentation from a connection accepted without one.

This is a reachable test-evidence gap, not a production TLS bypass or a reported execution failure. The current Kubernetes runner strictly applies this fixture tree and explicitly registers LdapAuthTls. The CA-only sibling expects a successful bind without a configured client certificate against the same listener. Locally, the compose fixture already uses a distinct port-33637 server configured with `demand`, showing the intended boundary without requiring another Gateway or Controller. Gateway LDAP TLS construction separately selects `with_client_auth_cert` or `with_no_client_auth`; existing material-binding unit tests verify reference identity but do not establish this Kubernetes server requirement.

Give the Kubernetes mTLS fixture a genuinely certificate-required endpoint, using the existing synthetic CA, client identity and seeded users. Keep CA-only LDAPS and STARTTLS on their existing compatible endpoint. Wire the existing mTLS route/plugin to the required endpoint and add a narrowly scoped no-client-identity control that must fail while the matching identity succeeds. Review preparation/readiness and run-owned cleanup for the additional dependency within the existing shared topology. Merely renaming the case or enabling certificate requirements on the shared listener would respectively retain the gap or break the CA-only sibling.

Preserve all four case identities and assertions, strict Secret-reference preparation, wrong-CA fail-closed behavior, existing deadlines, credential handling and canonical outbound TLS resolution. No production TLS changes, Gateway rustls support, logging, metrics, generic test framework or extra request-body ownership is needed.

The original LDAP egress guard already records an independent required-mTLS with/without-identity matrix; that standalone coverage remains valuable and is not claimed absent. It does not make this separately registered Kubernetes assertion prove its stated prerequisite. ARCH283 concerns classifying negative probe failures and has a different acceptance owner. Future verification must establish the required endpoint rejects missing identity and accepts the matching identity in Kubernetes, while CA-only and STARTTLS siblings still pass. Static repository review only: external image internals and live handshake outcomes were not executed or fetched.
