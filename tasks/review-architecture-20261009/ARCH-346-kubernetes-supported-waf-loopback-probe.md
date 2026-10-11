---
id: ARCH-346
status: candidate
kind: behavior-defect
priority: P2
partition: PART-455
analyst: /root/analyze_conversion
baseline: 3055a4b38a90022d9bb9e5ae6375e8caf725111ebcefe162779de8218585c3ff
---
# Establish the supported WAF probe's loopback identity in the Kubernetes topology

The supported CRS matrix requires an Apache-style probe to have a canonical loopback client identity, but its request builder leaves that identity dependent on the test process sharing the Gateway's network namespace. The registered Kubernetes WafDynamic execution runs the client and Gateway in separate Pods. A correctly functioning Gateway can therefore fail this phase because the test never established its own required precondition.

## Current ownership and witness

In waf_supported_wire.rs:157-222, status_probe(dummy=true) sends an HTTP/1.1 root request with the Apache user-agent and an attack cookie, then reads its uniquely correlated canonical access log. It explicitly rejects a non-loopback request_info.remote_ip with "Apache probe did not satisfy loopback/root conditions". The request supplies no forwarded client address. matrix calls this probe unconditionally at line 354, including strict On, strict DetectionOnly, standard On and retained/recovered phases.

This is an executable Kubernetes path: k8s_dynamic_scenarios.tsv registers WafDynamic, its runner executes test_client inside the test-client Pod with the Gateway Service DNS target, and run_supported_segment applies the supported fixtures and calls those actual phase suites. The shipped client and Gateway Deployments are separate namespace/network owners. For an ordinary IPv4 Pod-to-Service invocation, the accepted peer is a non-loopback Pod or node address. The canonical Gateway request filter resolves remote_ip through RealIpExtractor; without a forwarded header, that extractor retains the peer even when it is trusted. TestContext's client installs no default forwarded header. Consequently, after the earlier status and finding checks succeed, this probe still returns its precondition error. This is a static witness, not a claimed observed cluster incident or a guarantee about every customized topology.

The loopback check has real purpose. The supported package README and named-profile guard explicitly exclude original 905100/905110 because their audit-suppression controls remain unsupported. The checked-in upstream 905110 source requires loopback, the Apache user-agent and the root request line. The probe combines those conditions with a cookie that must still produce retained 933200 and 949110 findings. Dropping the loopback check would weaken evidence that the excluded exception cannot bypass protection. This finding does not request implementing those unsupported controls or changing the selected rule package.

## Bounded correction and alternatives

Make the test-owned request establish its required canonical identity through the existing real-IP fixture contract. The Kubernetes base EGC already selects X-Forwarded-For, trusts its declared private proxy ranges and 127/8, and uses recursive extraction. A narrow forwarded-loopback header on this probe can therefore establish the intended REMOTE_ADDR through the actual Gateway resolver. Keep the existing logged loopback/root/H1 assertions so an untrusted peer or incompatible fixture still fails visibly. Verify the required trust precondition rather than adding broad trust, rewriting production remote_ip, or assuming the header always wins.

An alternative is a dedicated loopback-origin fixture inside the Gateway network namespace, but it adds deployment ownership that the existing trusted-header mechanism avoids. A blanket Kubernetes skip or weaker arbitrary-client probe would not preserve the original exclusion predicate.

Keep every current wire status, mode, retained attack/scoring finding, intervention, omitted-finding, payload, trace and always-run cleanup assertion. Do not change WAF budgets, body storage/streaming, supported BoringSSL behavior, canonical access-log ownership, Controller workqueues or production logging. ARCH304 concerns missing execution ownership for other scenarios; this scenario is already registered. ARCH264/265/266 do not establish this request identity.

Future verification should exercise a non-loopback trusted peer and assert the probe's canonical logged address, plus an untrusted-peer negative case that cannot manufacture loopback. Retain local coverage and run the existing supported phases in Kubernetes. The actual fixture-parity test proves bundle equality/strict compilation, not this network precondition. No execution or mutation occurred; no performance or production-security claim is made.
