# Clarify Stream Plugin UDP Architecture Coverage

## Meta

- Status: pending
- Priority: P2
- Category: documentation investigation

## Evidence and scope

`skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md`
references this separate investigation. Its introduction in commit `d00e4be7` omitted
the task file, leaving the documentation link broken.

The UDP descriptions in `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
(lines 211, 248, 288 and 342 at `b297af6e`) need a consistent account of actual
stream-plugin coverage. The runtime conclusion has not been verified by this task's
creation. Existing prose and the listener decision are evidence to investigate,
not permission to infer or implement UDP support.

## Acceptance

- Read current repository rules, architecture and relevant review knowledge.
- Trace the actual UDP entry points, stream-plugin dispatch and protocol-specific exceptions.
- Reconcile conflicting statements in the owning architecture documentation, preserving the listener admission decision's distinct scope.
- Obtain independent review and pass the agent-documentation validator.
- Do not add UDP functionality or modify existing integration tests. If new integration coverage is needed, record it as a separate task for later.
