# GRPCRoute delegation ParentRef collision

The Gateway and delegating parent GRPCRoute deliberately share the
`edgion-test/parentref-collision-grpc` coordinate. The child has only an explicit
`kind: GRPCRoute` reverse-lock parentRef at that coordinate. Direct traffic on
the Gateway's wildcard listener must remain unmatched, while traffic through the
parent's sole hostname reaches the child's method. The Gateway in `bootstrap/`
must be loaded before Gateway startup because listener ports are not bound
dynamically. The scenario leaves that Gateway intact, clears its kind-qualified
GRPCRoutes, applies `initial/`, and then applies `updates/`, which changes only
the child generation and method.
