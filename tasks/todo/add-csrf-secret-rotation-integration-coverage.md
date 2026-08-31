# Add CSRF Secret-rotation Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | integration-test |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0070; not D0041 |

## AI guide summary

Add a standalone dynamic integration scenario that proves CSRF signing-key generations replace authoritatively across Secret rotation, deletion, and recovery.

## Requirements

- Add new scenario files rather than rewriting the existing CSRF integration scenario.
- Add only the minimal existing registry, manifest, or listener wiring required to make the new scenario runnable.
- Obtain a token signed by the original Secret, rotate the Secret, and wait for the new generation through readiness or phase polling with no fixed sleep.
- Prove the old token receives the existing 401 response and a newly issued token succeeds.
- Delete the Secret, wait for propagation, and prove the plugin's existing unresolved-key path fails closed with 500.
- Restore the Secret, wait for propagation, and prove token issuance and validation recover.
- Keep token/key values out of logs, test failure messages, snapshots, and committed fixtures.

## Acceptance criteria

- The standalone scenario covers rotation, old-token rejection, new-token success, deletion fail-close, and recovery without fixed sleeps.
- The existing CSRF scenario remains unchanged except for narrowly necessary shared registration that cannot live with the new scenario.
- Targeted integration execution, frozen-suite policy checks applicable at implementation time, workspace checks, and repository guards pass.
