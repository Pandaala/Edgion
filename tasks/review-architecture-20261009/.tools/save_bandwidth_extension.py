"""Apply only the independently approved ARCH280 merged body, hash-bound."""
import hashlib
import json
import record as r

state=r.state()
entry=state['candidates']['ARCH-280']
path=r.RUN/entry['path']
assert r.digest(path)=='b559635ece26278be60e73a70d0da771c29d2b095eb0b6e5ce30cb16167a2d37'
body=path.read_text().split('---',2)[2]
body=body.replace('# Bind LB assertions', '# Bind metric assertions',1)
body=body.replace('Four active LB suites judge current behavior', 'Four active LB suites and the BandwidthLimit metrics case judge current behavior',1).replace('before the policy verdict.', 'before the behavior verdict.',1)
insert='''`BandwidthLimitTestSuite::test_metrics_verification` in `bandwidth_limit/bandwidth_limit.rs` sends a 51,200-byte POST but checks neither its HTTP status nor the result of reading its response body. Sending errors and metric-fetch errors already fail. After a fixed 500 ms sleep, it fetches all backend metrics, keeps every series containing `latency_ms` and passes when their lifetime maximum is at least 800 ms. The generated `_test_key` is unused, and the claimed route filter is absent. Its immediately preceding functional-compliance case can leave a qualifying slow sample; a later completed HTTP error or failed body read can then pass this metrics verdict using the prior sample. The case need not even observe a fresh attributed increment. This is a static accepted-outcome witness, not an executed failure or proof that the entire suite passes.

Both local and Kubernetes BandwidthLimit fixture sets configure the same compliance, metrics and selected-BackendRef cases against `test-http`, with rates of 100 KiB/s, 50 KiB/s and 100 KiB/s respectively. Their shared `edgion-plugins-gateway` carries the actual `metrics-test-type: latency` annotation. The similarly named annotation on the metrics plugin resource is not the enabling owner. The published listener projection and terminal logging read the Gateway annotation, and test-mode latency measures elapsed time from `upstream_start_time` until terminal recording. Compliance traffic can therefore contribute the prior sample. The plugin remains deliberately stateless and per response stream; transport, backend work and scheduling also contribute to elapsed time, so this observation is not isolated throughput evidence.

'''
body=body.replace("The helper's public aggregate methods",insert+"The helper's public aggregate methods",1)
insert='''For the BandwidthLimit metrics case, take the baseline before its POST, require the expected successful status and a successfully completed response of the expected payload size, then poll under the existing deadline for exactly one new attributed terminal backend increment. Evaluate the existing 800 ms latency threshold only on positive count deltas from that window. Preserve the 51,200-byte request, the compliance and BackendRef cases and their body/duration checks, all three identities/order, current fixtures and thirty-second case deadline. Use the fixture-authoritative exposed Gateway, backend Service, protocol and status identities and the complete existing test-data series identity. The current parser does not retain route labels, so a route name in an error message is not attribution. Reject ambiguous/excess traffic rather than using a global maximum; do not infer a new per-request key from the unused generated string.

'''
body=body.replace('Reuse the existing raw `BackendMetric`',insert+'Reuse the existing raw `BackendMetric`',1)
body=body.replace('No matching current candidate was found', 'The PART460 supplement adds the independently inspected BandwidthLimit implementation, all three case bodies, both complete fixture sets, Gateway annotation publication and terminal latency producer to this same measured-window cause. The original per-stream bandwidth and no-runtime-tracing decisions remain intact. No matching separate candidate was found',1)
body=body.replace('Valid fresh hash/adaptive windows must pass under supported topology.', 'Bandwidth verification must additionally reject a prior slow sample followed by a current HTTP error, body-read failure, missing increment or fresh below-threshold latency, while accepting a complete successful response with its own qualifying attributed increment. Valid fresh hash/adaptive/bandwidth windows must pass under supported topology.',1)
assert hashlib.sha256(body.encode()).hexdigest()=='67ddd467a4a08df83e68ed98fb1ad55cb2e828c5c0a6460339e6c4b24bb2046e'
rows=[
 ('edgion-tests/src/client/suites/edgion_plugins/bandwidth_limit/bandwidth_limit.rs','d8900c62','Full all three actual bodies/order/deadline and lifetime latency predicate.'),
 ('edgion-tests/src/client/suites/edgion_plugins/bandwidth_limit/mod.rs','dc244f47','Full export.'),
 ('edgion-tests/src/client/suites/edgion_plugins/bandwidth_limit/plan.md','6e03542a','Full stale annotation/key plan versus actual owner.'),
 ('edgion-gateway/src/runtime/store/listener_gateway_info.rs','66b0123f','210-246 cold Gateway annotation publication.'),
 ('edgion-gateway/src/routes/http/proxy_http/pg_logging.rs','972f72ec','1-90,245-295,331-371 terminal backend and latency recording.'),
 ('edgion-gateway/src/observe/metrics/test_metrics.rs','8b043e7e','Full test-mode representation and serialization.'),
 ('edgion-gateway/src/observe/metrics/registry.rs','a53d0512','544-586 backend labels and increment.'),
 ('edgion-tests/src/client/metrics_helper.rs','6a645f91','Retained unchanged full implementation; fresh1-210/290-345 exact identities and no route retention.'),
 ('edgion-gateway/src/plugins/http/bandwidth_limit/plugin.rs','e5b87698','Full stateless per-stream implementation.'),
 ('edgion-gateway/tests/unit/plugins/http/bandwidth_limit/plugin_test.rs','5ff4d753','Full actual three test bodies.'),
 ('edgion-resources/src/resources/edgion_plugins/plugin_configs/bandwidth_limit.rs','c98a1288','Full parser/validator; kb is1024.'),
 ('edgion-tests/src/client/framework.rs','dd7b89a2','648-682 serial runner; retained536-611 deadline.'),
 ('edgion-tests/src/server/test_server.rs','6c38b285','815-846 actual POST echo; expected output size must account for framing.'),
 ('skills/04-review/testing/regression-guards/integration-lb-metrics-require-scoped-deltas.md','610f5799','Full original scoped delta and attribution contract.'),
 ('skills/04-review/rate-limit/intentional-design/bandwidth-limit-per-stream.md','2d60b3df','Full original stateless per-stream contract.'),
 ('skills/04-review/observability/intentional-design/bandwidth-limit-invalid-rate-no-tracing.md','20ab51c8','1-3 conclusion/core rationale; no new request tracing/metric.'),
 ('skills/03-coding/observability/01-metrics.md','7a498385','270-289 integration metric exception/restraint.'),
]
supports={}
for p,prefix,scope in rows:
    assert r.digest(r.REPO/p).startswith(prefix),p
    supports[p]=scope
for tree in ['integration','k8s']:
    for filename,prefix in [('01_BandwidthLimit_compliance.yaml','4530d276'),('02_BandwidthLimit_metrics.yaml','12d73545'),('03_BandwidthLimit_backend.yaml','14341e25'),('HTTPRoute_bandwidth_limit.yaml','a31b6aba')]:
        p='edgion-tests/'+tree+'/conf/EdgionPlugins/BandwidthLimit/'+filename
        assert r.digest(r.REPO/p).startswith(prefix),p
        supports[p]='Full actual '+tree+' selected rate, backend and case fixture.'
    p='edgion-tests/'+tree+'/conf/EdgionPlugins/base/Gateway.yaml'
    assert r.digest(r.REPO/p).startswith('f4b779f4'),p
    supports[p]='Full actual Gateway-owned latency enablement.'
lb=[('lb_consistenthash.rs','40834efd'),('lb_ewma.rs','8fabcb80'),('lb_leastconn.rs','d2adae19'),('lb_leastconn_retry_on_status.rs','02737ec7'),('lb_roundrobin.rs','f73adca9'),('lb_ewma_retry_on_status.rs','e2656883'),('informational_retry.rs','e31a22b0'),('mod.rs','4e61d4fc'),('partially_invalid.rs','49f50f1f')]
for filename,prefix in lb:
    p='edgion-tests/src/client/suites/http_route/backend/'+filename
    assert r.digest(r.REPO/p).startswith(prefix),p
    supports[p]='Personally retained full unchanged implementation and actual test bodies; current hash/assertion seams reread. Zero new coverage.'
if entry.get('verification'):
    entry.setdefault('verification_history',[]).append(entry['verification'])
old=r.RUN/'VERIFICATION-ARCH-280.json'
archive=r.RUN/'VERIFICATION-ARCH-280-PRE-EXTENSION-20261011-D.json'
if old.exists():
    assert not archive.exists()
    archive.write_bytes(old.read_bytes())
r.save(state)
r.confirm('ARCH-280','/root/verify_ip_union','b559635ece26278be60e73a70d0da771c29d2b095eb0b6e5ce30cb16167a2d37',
 'e4ecaeb88e93af030186580c9a545f9b8c0109edfb4c6a5275fdc54a84d1206c',
 ['Prior LB measured-window claim remains current; nine retained full primary hashes unchanged and current verdict seams independently reread.',
  'Bandwidth test_metrics_verification shares window cause: send and fetch failures fail, but status/body outcomes are ignored and lifetime all-series maximum can reflect preceding compliance traffic.',
  'Actual Gateway annotation enables latency; no plugin-annotation enablement defect or isolated throughput claim.'],supports,replacement_body=body)
state=r.state()
state['candidates']['ARCH-280'].update(revalidation_pending=False,revalidated_baseline=state['current_baseline'])
state.get('pending_supplements',{}).pop('ARCH-280',None)
r.save(state)
print('ARCH280 exact approved merged claim saved; no new issue or primary coverage.')
