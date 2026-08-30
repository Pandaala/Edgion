# Task-History References Must Resolve

Source comments and long-lived Skills must not depend on task paths that no
longer exist. A task can preserve useful design history while its branch is in
progress, but implementation rationale that outlives the task belongs in a
canonical architecture, design-rule, or review document.

When a completed task reference breaks:

1. Point durable behavioral contracts to `skills/` or to the executable test
   that enforces them.
2. Make local implementation comments self-contained when the removed task
   only supplied numbering such as “R-5” or “Phase 2b”.
3. Do not recreate an obsolete task tree solely to satisfy links, and do not
   delete surviving design evidence.
4. Follow `skills/07-tasks/SKILL.md`: active work lives under `tasks/pending/`,
   supported status values include `completed` but not `done`, and completed
   records are archived by Git history after merge rather than moved to a
   separate done directory.

Documentation validation is necessary but not sufficient: plain-text paths in
Rust and shell comments may not be interpreted as Markdown links, so search
the removed path literally across the repository as part of closure.
