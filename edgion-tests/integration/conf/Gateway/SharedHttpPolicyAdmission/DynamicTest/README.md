# Shared HTTP policy admission

The shared topology starts two compatible Gateways from different GatewayClasses on one HTTP listener and a healthy sibling listener. The scenario changes B to an incompatible H2 declaration, checks partial rejection and traffic withdrawal, restores B, then deletes blocker A and checks dependency-driven recovery. B's ordinary Route remains accepted with both parsed attachments throughout; traffic exclusion is enforced by listener matching. All phases restore the initial configuration on success and failure.

HTTP probes verify logical admission on already-bound services. They do not claim that an existing physical H2 policy changed without restart.

Bootstrap also declares HTTPS with an absent certificate Secret. The scenario creates only that Secret, requires same-port HTTPS recovery with the same Gateway PID, and removes the temporary Secret for repeatability.

Startup reference validation exempts only this Gateway's exact missing certificate reference for this suite. Resource synchronization, all other missing references, and the scenario's explicit absence checks remain enforced.

An additional Gateway references an absent EGC and declares an incompatible policy on the same HTTP key. It must not block the usable cross-class owners.
