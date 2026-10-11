---
id: ARCH-377
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-498
analyst: /root/analyze_outbound
baseline: 8f1c53149be48bb3de89f8cf43a7f08d7ecff602717ec079f6b7ee316705c306
---

# Require the KeyAuth anonymous cases to observe their distinct identities

The two existing anonymous KeyAuth cases treat their identity assertions as optional success messages. `test_anonymous_access_no_key` requires HTTP 200, reads the `/headers` body, and returns a passed result both when `x-anonymous-consumer` is found and when it is missing. `test_anonymous_with_valid_key` likewise returns success whether the echo contains the authenticated username `jack` or not. The source comments explicitly distinguish anonymous access from a valid key's authenticated identity, but the outcome boundary establishes only HTTP availability.

A concrete witness needs no transport failure: a complete HTTP 200 JSON echo with an empty `headers` object passes both cases. Returning `X-Consumer-Username: anonymous-user` for the valid-key request also passes its fallback branch. Consequently, a regression confined to identity publication or valid-key selection on the anonymous-enabled route can evade these two assertions. This does not establish a production authentication bypass, an observed integration incident, or that every case in the suite would pass during a wider regression.

The required identities already have real owners. Both local and Kubernetes `02_EdgionPlugins_default_key-auth-anonymous.yaml` configure `anonymous: anonymous-user`, the actual API-key Secret, and `X-Consumer-Username` in `upstreamHeaderFields`. The Secret maps the test's valid Jack key to username `jack`; both anonymous routes select this plugin. Production `KeyAuth::run_request` writes username `anonymous-user` and `X-Anonymous-Consumer: true` on the missing-key branch. A valid key instead selects its resolved metadata and writes username `jack`. The Gateway's actual unit bodies independently assert anonymous identity writes and authenticated metadata forwarding. The existing test server's `/headers` handler returns an exact lowercase header-name map, so observing these values requires no additional instrumentation.

Keep the two original cases and require their declared identities through that existing JSON envelope. Fail body-read, JSON, envelope and identity mismatches. The no-key request must observe `x-anonymous-consumer == "true"` and `x-consumer-username == "anonymous-user"`; the valid-key request must observe `x-consumer-username == "jack"` and must not observe an affirmative anonymous marker. Do not supply client identity headers to manufacture the expected echo. Retain HTTP 200, logical authority, selected context listener, current request deadline, fixture references and all neighboring cases. Two local assertions suffice; a generic authentication-test framework is unnecessary.

The alternative of accepting HTTP 200 preserves availability coverage but cannot distinguish these branches, because anonymous access deliberately allows the same status. This cause is distinct from ARCH348's unreadable-body-to-empty negative assertion: the decisive witness here is a successfully read response with missing or wrong positive identity. Preserve secret-safe diagnostics by reporting the operation and missing/mismatched header names without dumping the complete echoed header map or API key. No production admission, fallback, logging, metrics, body storage, TLS or workqueue change is proposed. This review inspected current implementations, both fixture trees, real unit bodies and registration without executing tests, services or network calls.
