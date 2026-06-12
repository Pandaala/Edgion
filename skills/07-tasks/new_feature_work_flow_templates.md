# New Feature Work Flow Templates

Companion templates for `new_feature_work_flow.skill`. The main workflow only describes the process; this file fixes the output format.

## 0. Requirement summary template

```markdown
## Requirement summary
- Problem to solve:
- Expected behavior:
- Explicitly out of scope:

## Initial impact assessment
- Possibly affected modules:
- Key risks:
- Need a tasks document: yes / no

## Related Skills
- `skills/...`
- `skills/...`
```

## 1. Solution-assessment template

```markdown
## Solution assessment

| Dimension | Option A | Option B |
|------|--------|--------|
| Change difficulty |  |  |
| Impact scope |  |  |
| Safety |  |  |
| Implementation value |  |  |
| Test difficulty |  |  |
| Compatibility |  |  |
| Rollback ability |  |  |

### Option A (recommended)
- Core idea:
- Change points:
- Strengths:
- Risks:
- Test requirements:

### Option B (alternative)
- Core idea:
- Reason for rejection:

## Assessment conclusion
- Recommended option:
- Reason:
- Open assumptions:
- Main reasons rejected options were dropped:
```

Must not omit:

- Safety
- Compatibility
- Open assumptions

## 2. Code-level design template

```markdown
## Code change plan

### File 1
- Path:
- Change content:
- Key functions:

### File 2
- Path:
- Change content:

## Data flow
- Where the request/config enters
- Where it is transformed
- Where it is consumed

## Test design
- Unit tests:
- Integration tests:

## Risks and compatibility
- Compatibility handling:
- Failure rollback points:
```

Minimum design requirements:

- Every file to be changed lists its responsibility
- Every new behavior is matched by at least one test point
- For configuration, annotation, Schema, or resource-field changes, mark the corresponding source document

## 3. Design-revision conclusion template

```markdown
## Design revision conclusion
- Adopted:
- Not adopted:
- Reason:
```

## 4. Implementation-result template

```markdown
## Implementation Result
- Modified files:
- Core implementation:
- Tests added:
- Issues encountered:
```

## 5. Code-review convergence template

```markdown
## Code Review Resolution
- Must Fix resolved:
- Should Fix not resolved:
- Reason:
- Acceptable Risk:
```

## 6. Test-coverage analysis template

```markdown
## Test coverage analysis

| Change point | Existing unit tests | Added unit tests | Existing integration | Added integration | Residual risk |
|--------|----------|----------|----------|----------|----------|

## Test reinforcement conclusion
- Must-add tests added:
- Reason for items intentionally not added:
```

## 7. Test-execution record template

```markdown
## Test execution record

| Command | Result | Notes |
|------|------|------|
| `cargo fmt --all -- --check` | pass/fail |  |
| `cargo clippy --all-targets` | pass/fail |  |
| `cargo test --all` | pass/fail |  |
| `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r ... -i ...` | pass/fail |  |

## Failure attribution
- This change:
- Environment issue:
- Pre-existing failure:
```

## 8. Wrap-up summary template

```markdown
## Wrap-up summary
- Solution:
- Core changes:
- Key revisions:
- Tests:
- Risks:
- Follow-ups:
```

## Failure-record template

```markdown
## Current blocker
- Phase:
- Symptom:
- Root-cause assessment:
- Attempts made:
- Next action:
```
