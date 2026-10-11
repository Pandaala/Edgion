---
id: ARCH-386
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-506
analyst: /root/analyze_conversion
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Register the existing ProxyRewrite combined host and header assertion

`ProxyRewriteTestSuite::test_combined` in `edgion-tests/src/client/suites/edgion_plugins/proxy_rewrite/proxy_rewrite.rs:313-391` sends distinctive headers and checks Host replacement, header addition, a `$uri`-derived set value and removal of both `X-Debug` and `X-Internal-Token`. However, `test_cases()` at the end of that same file returns nine other cases and never includes `Self::test_combined()`. Both local and Kubernetes suite registrations reach this vector, so the implemented combination assertion is disconnected from normal execution.

The actual fixture trees contain `proxy-rewrite-combined` with `host: backend.internal.svc`, add/set/remove entries and a `/combined` HTTPRoute rule selecting that plugin. The nine registered requests use other route prefixes; none invokes this rule. The separate ResponseRewrite combined case selects `response-rewrite.example.com` and tests a different plugin. This is not a conclusion from an absent inventory line: the actual case-producing vector, runner iteration, suite invocation and authored routing inputs establish the missing execution edge.

A concrete conditional regression is removing Host replacement or header removal from the combined fixture while leaving the other eight plugin resources unchanged. The registered ProxyRewrite suite still exercises its URI, query encoding, capture and path-parameter cases without reaching the broken combination. Its dormant case would detect the changed Host or retained sent removal markers if registered. Existing pure encoding tests and the separate canonical header-append regression remain valuable; this finding does not claim all ProxyRewrite coverage is absent or a production regression has occurred.

Add the existing combined case to this suite's case vector. Preserve its positive status/read checks and all current Host/add/set/remove assertions, as well as the nine already registered cases and their order where relevant. Reuse the existing fixtures, injected listener context, HTTP client, echo parser and test runner. Keep the same shared Controller/Gateway topology. Deleting the dormant test or marking its fixture as unused would erase the meaningful contract rather than restore it; a new generic case registry is unnecessary.

Future verification should show the combined case appears exactly once in both targeted and aggregate suite results and fails when each claimed Host/header effect is independently removed. Preserve the production encoding unit tests, genuine multi-value append behavior and existing local/Kubernetes integration coverage. No production rewrite, schema, logging, metrics, credential policy or body-storage changes are required. ARCH037 concerns production token substitution and ARCH304 concerns Kubernetes dynamic-scenario invocation parity; their owners and remedies differ. This review read the full primary implementation and decisive source/fixture/test seams without executing tests or changing files.
