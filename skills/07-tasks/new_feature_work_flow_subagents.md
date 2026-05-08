# New Feature Work Flow Subagents

Subagent collaboration constraints used by `new_feature_work_flow.skill`. The goal is to reduce out-of-scope implementation, duplicated design, and low-quality reviews.

## General rules

- The main agent is responsible for final convergence and must not accept subagent conclusions unconditionally
- Multiple subagents must not modify the same set of files concurrently
- If a subagent finds the design unworkable, it must report the blocker rather than change direction on its own
- Subagent outputs must be conclusive and actionable; avoid vague suggestions

## Design Review Subagent

Input:

- Summary of the original requirement
- Summary of the current design
- Related skill paths
- Designated risk-focus areas

Goal:

- Find design gaps, omissions, and places that violate the existing architecture

Forbidden:

- Do not rewrite the entire design
- Do not start implementation directly
- Do not give vague suggestions without conclusions

Output template:

```markdown
## Design Review Findings

### Must Fix
- [severity] Issue:
  - Reason:
  - Suggested fix:

### Nice to Have
- Issue:
  - Suggestion:

### Assumptions To Verify
- ...
```

## Implementation Subagent

Input:

- The finalized design
- Explicit file ownership
- Explicit out-of-bounds areas

Goal:

- Complete the implementation and directly related unit tests within the assigned file scope

Forbidden:

- Do not change design goals on your own
- Do not refactor unrelated files opportunistically
- Do not modify unauthorized files

Output template:

```markdown
## Implementation Result
- Modified files:
- Core implementation:
- Tests added:
- Issues encountered:
```

## Code Review Subagent

Input:

- The final implementation diff
- Summary of the requirement and design
- Focus modules

Goal:

- Identify residual issues, behavioral deviations, and hidden risks after implementation

Forbidden:

- Do not relitigate the design discussion of step 3
- Do not give purely stylistic suggestions unless they affect correctness, maintainability, or security

Output template:

```markdown
## Code Review Findings

### Must Fix
- [file/logic] Issue:
  - Impact:
  - Suggested fix:

### Should Fix
- ...

### Acceptable Risk
- ...
```

## Main-agent convergence rules

- `Must Fix` items must be fixed by default
- For unfixed `Should Fix` items, record the reason
- `Acceptable Risk` items must enter the final summary
- If a subagent's conclusion contradicts the code fact, the main agent's recheck takes precedence
