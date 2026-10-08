# Istio Ambient + Edgion Integration Study

- Date: 2026-09-25
- Status: Initial architecture assessment; runtime compatibility unverified
- Parent: [Task 08](tasks/08-ambient-feasibility.md)

## Recommendation

Investigate upstream Istio Ambient infrastructure with Edgion enrolled as an
ordinary application proxy. Keep istiod, istio-cni, and ztunnel intact for the
first experiment. Explicitly route callers through Edgion for L7 governance.
This approach tests whether existing Edgion traffic processing can run over
Ambient transport without implementing HBONE in Edgion.

This is a proposed integration topology, not a verified supported deployment.
It does not require completing the independent Edgion certificate issuer tasks.

## Responsibility split

| Concern | Proposed owner |
| --- | --- |
| Pod enrollment, capture, and network lifecycle | Istio CNI and ztunnel |
| Workload certificates, trust, and transport authorization | Istio CA/control plane and ztunnel |
| Business Gateway resources and configuration sync | Edgion Controller |
| HTTP/gRPC routing and application-layer authentication | Edgion Gateway |
| Public-facing HTTPS certificate, if exposed externally | Separate existing Edgion frontend TLS configuration |

Use Istio's certificate lifecycle for Ambient transport without cert-manager.
Do not issue duplicate Mesh transport certificates from Edgion for this path.
Application TLS, public ingress TLS, and Edgion conf_sync credentials retain their
own purposes and owners. The independent self-managed Edgion TLS design remains
an alternative track; no runtime implementation has started in either track.

## Candidate first topology

```text
Enrolled caller
  -> source ztunnel
  == HBONE/mTLS ==> ztunnel serving the Edgion Pod
  -> Edgion HTTP listener: routing and application authorization
  -> ztunnel serving the Edgion Pod
  == HBONE/mTLS ==> ztunnel serving the backend Pod
  -> ordinary HTTP backend
```

All three application participants are explicitly enrolled. Callers use an
Edgion entry Service; Edgion backendRefs resolve to separate real backend
Services/endpoints. This prevents a Service redirection loop. HTTP/gRPC protocols
and listener configuration remain Edgion concerns.

The first experiment omits Istio waypoints. Do not assign a waypoint GatewayClass
to Edgion or assume a waypoint label makes it compatible. Enrolling a Gateway Pod
as an ordinary workload must itself be verified, including any chart labels,
host-network settings, excluded ports, probes, and actual socket destinations.

## Identity and enforcement boundaries

Edgion terminates the incoming application connection and opens a new upstream
connection. Expect the backend's transport peer identity to be Edgion's ServiceAccount,
not the original caller's identity. Nor should Edgion be assumed to receive the
caller's verified SPIFFE identity automatically over the local plaintext hop.

Use Istio transport policies to control allowed workload peers and existing Edgion
application authentication for user/request identities. Untrusted forwarded headers
must not substitute for a verified Mesh identity.

Requiring traffic through Edgion needs a backend authorization rule allowing the
intended Edgion identity and rejecting direct caller access. STRICT mTLS alone
does not establish that routing restriction. Test both Service-IP and Pod-IP
bypass attempts, plus non-enrolled callers.

## Prototype sequence and evidence

1. Pin Kubernetes, CNI, Istio, and Edgion versions; use a disposable multi-node
   Linux cluster. Record manifests and reproducible commands.
2. Verify Ambient mTLS between two ordinary HTTP workloads before introducing
   Edgion. Establish workload identities and negative authorization tests.
3. Enroll the Edgion Gateway Pod as an ordinary workload with a dedicated
   ServiceAccount. Route one HTTP and one gRPC example explicitly through it.
4. Verify capture of Edgion's actual EndpointSlice-selected Pod-IP connections,
   not only a separate test client connecting to a ClusterIP.
5. Demonstrate an Edgion L7 rule, secure transport on both mesh legs, and rejection
   of direct backend access. Record the identity seen on each leg. HTTP success
   alone is not encryption evidence; collect ztunnel connection/security evidence.
6. Check probes, backend health checks, conf_sync connectivity, long-lived streams,
   certificate renewal, Gateway replica changes, and ztunnel restart behavior.
7. Report whether configuration alone works. Open minimal compatibility fixes
   only for reproduced issues; do not preemptively implement a new Mesh mode.

Same-node and cross-node behavior must both be exercised. External ingress is a
separate test: the external-to-Edgion hop needs its own TLS/authentication policy.
Safe removal must retain intended access restrictions instead of silently opening
a direct backend path.

## Later options

- Keep Edgion as an explicitly addressed business gateway if this meets the need.
- If original service addresses must transparently select Edgion for L7, study a
  genuine waypoint implementation separately: HBONE, identity, destination routing,
  enrollment, and control-plane contracts all need validation.
- Consider replacing istiod with an Edgion-managed ztunnel control plane only if
  a measured requirement justifies taking over configuration and CA interfaces.

## Evidence and limits

Official sources consulted on 2026-09-25:

- [Workload enrollment](https://istio.io/latest/docs/ambient/usage/add-workloads/):
  Ambient enrollment is controlled through workload/namespace labels.
- [Data-plane routing](https://istio.io/latest/docs/ambient/architecture/data-plane/):
  enrolled workloads use transparent L4 interception; routing considers Service
  and Pod destinations. Applying this to Edgion is the hypothesis to test.
- [Traffic redirection](https://istio.io/latest/docs/ambient/architecture/traffic-redirection/):
  CNI and ztunnel cooperate in Pod network namespaces.
- [Ztunnel architecture](https://github.com/istio/istio/blob/master/architecture/ambient/ztunnel.md):
  workload identities, authorized certificate acquisition, and renewal.
- [Waypoint configuration](https://istio.io/latest/docs/ambient/usage/waypoint/):
  optional L7 proxies have explicit enrollment and traffic handling contracts.

No cluster experiment or fresh Edgion compatibility test was run for this report.
