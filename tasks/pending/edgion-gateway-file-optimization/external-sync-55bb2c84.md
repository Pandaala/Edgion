# External Sync — 55bb2c84

- Expected concurrent user commit `55bb2c84` arrived during B0232 plan review. It changes no existing Gateway source. It adds `edgion-gateway/tests/tls_passthrough_clienthello.rs`, which is a new pending external-test coverage row; authoritative inventory increases from 547 to 548 files and non-terminal coverage increases by one.
- Independent diagnostic `remote_7825_reconcile` verified the five integration paths below byte-for-byte against the commit and current HEAD. All are regular Git `100644`, filesystem `0644`, executable bits `000`. Only these manifest entries were added or updated; frozen count 3,121 to 3,124. No campaign integration edit or execution occurred.
  - Add `edgion-tests/integration/conf/HTTPRoute/Transport/10_h1_stalled_upload.yaml`: SHA-256 `da1c3a2bf95f7f902b34a0567c827d520a32b7b383c663f1fbd30968e1062fcd`.
  - Add `edgion-tests/integration/conf/HTTPRoute/Transport/10_h2_stalled_upload.yaml`: SHA-256 `f69d81dbd2c21975360c0cce75d23852b0aa21bbd9934a3ee4d5b9466d8bf5b8`.
  - Add `edgion-tests/src/client/suites/http_route/transport/downstream_timeout.rs`: SHA-256 `e6b1f0ff9b25e7daa3eaeddbd73c00960a3b5a9b0818ba0bd1957e17ab9d6a41`.
  - Update `edgion-tests/src/client/suites/http_route/transport/transport.rs`: SHA-256 `b800eaa8e372138c958e7900e69acd32cd91156da93403788afde739b6a22271`.
  - Update `edgion-tests/src/client/transport.rs`: SHA-256 `bbf3d00071236910b8a5d11f137b7f13115ead87ffe61e596ed189be09671eae`.
- The external downstream timeout and malformed ClientHello tasks are validly closed and deleted. Maintained knowledge records the actual H1/H2 stalled-upload and isolated production-callback socket boundaries and their unverified limits. External 74-case Transport/socket results remain historical and are not B0232 verification. Do not recreate the deleted tasks.
- The changed DSL Redis policy knowledge only reconciles completed-task storage; Redis policy and script semantics are unchanged. B0232's target blob and plan remain valid pending independent plan approval. User-owned handoff/WAF paths remain untouched; never push.
