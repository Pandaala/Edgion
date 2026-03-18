# Claude Project Memory

Use `AGENTS.md` in the repository root as the canonical project instruction file.

When more project context is needed:

- Start from `skills/SKILL.md`
- Load only the relevant domain `SKILL.md`
- Use `docs/` for human-facing background and multilingual explanations

Keep tool-specific behavior thin here. Project knowledge should stay centralized in `AGENTS.md` and `skills/`.
