# Historical Gateway API source schemas

`grpcroute-v1alpha2-v1.1.1-experimental.yaml` retains only the `v1alpha2`
version of the official [Gateway API v1.1.1 Experimental GRPCRoute CRD](https://github.com/kubernetes-sigs/gateway-api/blob/v1.1.1/config/crd/experimental/gateway.networking.k8s.io_grpcroutes.yaml).
It is embedded into the Controller's source-schema validator at build time.
This directory is not an installable Kubernetes CRD bundle.

The file was extracted by YAML parse and serialization without modifying the
selected version's `openAPIV3Schema`. SHA-256 of the committed extracted file:
`c21e440c618ad427edaecd2fe0e3644edf634237a50a77321cc0551d52719261`.

The v1.6.2 Experimental CRD bundle under `config/crd/gateway-api/` supplies
the selected TLSRoute, BackendTLSPolicy, TCPRoute, and UDPRoute alpha schemas.
