# CredentialInjector Dynamic Test

This scenario first verifies the finite custom Secret-group spellings on one
CredentialInjector, then runs the original Secret update/delete/recovery sequence.

Before Secret rotation, transient copies of the same plugin use omitted group,
explicit empty group, literal `core`, and omitted group again. Each copy includes
a distinct, non-sensitive request-header marker in the same plugin resource. A
successful upstream echo must contain both that marker and the initial bearer;
received configuration alone cannot satisfy the phase.

The Controller and Gateway must expose the exact authored group and phase
generation, and this Controller's `Accepted` and `ResolvedRefs` conditions must
observe that generation. Filesystem mode authors increasing generations;
Kubernetes mode uses server-assigned generations. Resource versions are not
required to match across Controller and Gateway. Bounded polling waits for
eventual convergence, followed by the unchanged initial suite and its Kubernetes
phase acknowledgement.

| Phase | Expected behavior |
|-------|-------------------|
| `initial` | The upstream receives the initial bearer value |
| `updated` | The upstream receives the rotated bearer value |
| `deleted` | The request fails closed and does not reach the upstream |

The original plugin spec is restored, including omitted group and removal of the
marker, before Secret rotation. Restoration is attempted even when an alias phase
fails; both phase and restoration errors are retained. The original Secret
sequence and its final restoration remain unchanged, so later suites see the
initial configuration. Process termination remains subject to the runner's
existing containment and cleanup.

This checks custom CredentialInjector Secret-group equivalence. It does not prove
native Gateway API group handling, direct Kubernetes admission, LinkSys kind
selection, cross-namespace grants or etcd parity. Local filesystem and Kubernetes
live results must be recorded separately.
