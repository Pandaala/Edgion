---
name: oncelock-global-singleton-deliberate-design
description: Use when reviewing findings that flag GLOBAL_CONFIG_CLIENT OnceLock as unable to release on hot reconnect; the singleton lifetime matches the Gateway process lifetime by design.
knowledge_type: intentional-design
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# OnceLock Global Singleton is Deliberate Design

**False-positive scenario**: `GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>>` cannot be released; flagged as "hot reconnect cannot reclaim the old ConfigClient".

**Reality**: within the Gateway process lifetime, the Controller endpoint connected to is fixed; **hot-switching the Controller is not a current feature requirement**. The `OnceLock` global singleton is a deliberate design decision. If hot reconnect is needed in the future, the entire config-sync architecture needs to be redesigned systematically rather than simply replaced with mutable state (which would introduce a host of concurrency-safety issues).

**Verdict**: deliberate design; not a memory issue.
