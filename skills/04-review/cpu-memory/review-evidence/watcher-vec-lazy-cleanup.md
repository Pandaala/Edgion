---
name: watcher-vec-lazy-cleanup
description: Use when reviewing findings that flag watchers RwLock<Vec<WatchClient>> for not shrinking immediately after disconnection, treating lazy cleanup as a memory growth issue.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Watcher Vec Lazy Cleanup is Sufficient

**False-positive scenario**: `watchers: RwLock<Vec<WatchClient<T>>>` does not shrink immediately after a disconnection; flagged as memory growth.

**Reality**: each new watcher registration runs `retain(|w| !w.sender.is_closed())` to clean disconnected old watchers; cleanup is timely as new and old watchers alternate. In the Controller scenario, watcher counts are limited (typically < 100); even if all accumulate it does not exceed 20KB. Adding retain to `notify_watchers` would acquire the write lock on every broadcast, instead increasing hot-path lock contention.

**Verdict**: lazy cleanup is sufficient; the current design is reasonable; not a memory issue.
