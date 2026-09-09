# HTTPRoute delegation ParentRef collision

The Gateway and delegating parent HTTPRoute deliberately share the
`edgion-test/parentref-collision-http` coordinate. The child has only an explicit
`kind: HTTPRoute` reverse-lock parentRef at that coordinate. Direct traffic on
the Gateway's wildcard listener must remain unmatched, while traffic through the
parent's sole hostname reaches the child's rule. The Gateway in `bootstrap/`
must be loaded before Gateway startup because listener ports are not bound
dynamically. The scenario leaves that Gateway intact, clears its kind-qualified
HTTPRoutes, applies `DynamicTest/initial/`, and then applies `DynamicTest/updates/`, which changes only
the child generation and path.


The bootstrap Gateway remains outside `DynamicTest/` so the main Kubernetes prepare phase binds the listener before Gateway startup. The initial route generation lives under `DynamicTest/initial` and is the reset baseline; update generations live under `DynamicTest/updates`, which the fixture-ownership guard excludes from the effective static configuration.
