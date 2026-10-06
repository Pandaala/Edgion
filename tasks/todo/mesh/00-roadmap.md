# Edgion Mesh Roadmap

## Current direction

Start with a business-owned Controller and Gateway deployment watching a fixed
namespace set. Reuse existing resources and proxy capabilities; the initial route
adds no production feature. The intended Mesh Controller then automates business
certificate issuance, renewal, and TLS/mTLS configuration without cert-manager,
reusing existing resources and Secret reload paths.

This direction supersedes the earlier integration-only decision in this document.
Controller-owned certificate automation is the intended next capability. Ambient
implementation, new Mesh CRDs, and automatic Service takeover remain undecided.

## Tracks

| Track | Status and scope |
| --- | --- |
| [Business Mesh Gateway](business-mesh/README.md) | Current design investigation; deployment validation pending |
| [Overall specification](business-mesh/01-spec.md) | Existing-capability baseline, self-managed certificates, and proposed verification |
| [Ambient investigation](business-mesh/02-ambient.md) | Additional transport and identity responsibilities; future decision |
| [Linkerd integration](01-linkerd-integration.md) | Earlier optional proposal; support and assumptions remain unverified |
| [Istio integration](02-istio-integration.md) | Earlier optional proposal; support and assumptions remain unverified |

The older integration proposals are reference tracks, not prerequisites for the
business Gateway baseline or evidence of current interoperability support.
