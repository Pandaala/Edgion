---
name: <rule-name-in-kebab-case>
description: Use when reviewing <topic area> findings; covers <one-line summary of what the rule decides>.
status: fixed | accepted-tradeoff | false-positive | rfc-divergence | wont-fix
finding_id: <e.g. backends-F010 / P3-g3-016 / http1-04>
closed: <YYYY-MM-DD>
---

<!--
Copy this file to `04-review/<topic>/<rule-name>.md` and fill in every `<...>` placeholder.
The finding ID lives in the `finding_id:` frontmatter field and the H1 title — NOT in the filename
(filenames are descriptive kebab-case slugs; the topic directory supplies the category context).

ONE FINDING = ONE FILE for the detailed rationale. Also add one concise coverage row to
`skills/04-review/CLOSED-FINDINGS.md`; that index points readers here and must not duplicate
the full rationale. When several findings are resolved concurrently, serialize the small
index updates after the independent files are ready.

Pick `<topic>` = the directory where sibling findings of the same family already live
(`find skills/04-review -mindepth 1 -maxdepth 1 -type d`); create the directory if the family is new.

The "Decision rule" shape is canonical for new entries — Conclusion-first, then rationale,
then explicit rejection of competing fixes, then re-evaluation triggers, then reference cases.

Existing files that use older shapes (scenarios-list, comparison matrix, etc.) do NOT need to
be retro-fitted unless a re-review actively touches them.
-->

# <Human-Readable Rule Title> — <VERDICT e.g. NOT-AN-ISSUE | DESIGN-TRADEOFF | ACCEPTED-RISK>

## Conclusion

<One or two sentences stating the verdict and what fix suggestions are not accepted.>

Fix suggestions of the form "<example rejected suggestion>" are **not accepted**.

## Core Rationale

**1. <Primary reason heading>**

<Explanation. Be specific: point to config options, docs pages, or source files.>

**2. <Secondary reason heading>**

<Explanation. Compare to industry precedent where applicable (e.g., Kong / Envoy / AWS SigV4).>

**3. <Tertiary reason heading — delete if not needed>**

<Explanation.>

## Fix Suggestions Not Accepted

- <Rejected fix 1> — <brief reason why it would cause harm or provide no value>
- <Rejected fix 2> — <brief reason>
- <Rejected fix 3> — <brief reason; delete row if not needed>

## Re-evaluation Triggers

Re-open this decision only if:

- <Concrete condition that would change the tradeoff, e.g., "a new transport mode is added that cannot rely on TLS">
- <Another concrete condition, e.g., "the threat model expands to include X">
- <Delete this row if not needed>

## Reference Cases

- <Issue / PR / audit date where this rule was first established>
- <Link to a related doc or source file, e.g., `docs/en/user-guide/...`>
- <Link to a related skill file, e.g., `skills/02-features/...`>
