---
name: access-log-json-serialization-per-request
description: Use when reviewing findings that flag AccessLogEntry::to_json() per-request String allocation as a hot-path issue and suggest buffer pools, Bytes, or deferred consumer-thread serialization.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Access Log JSON Serialization Allocating a New String per Request is Not an Issue

**False-positive scenario**: `edgion-gateway/src/observe/access_log/entry.rs:103-110` `AccessLogEntry::to_json()` runs `serde_json::to_string(self)` on the request task context; allocates a new owned `String` per request; flagged as "hot-path per-request alloc; should use a buffer pool / defer to consumer thread / switch to `Bytes`/`Arc<str>`".

**Reality**:

1. **Architecture is a channel-based async logger**: call chain `routes/http/proxy_http/pg_logging.rs:42-63` → `AccessLogger::send(String)` → `LocalFileWriter` internal `SyncSender<String>::try_send` (`link_sys/providers/local_file/data_sender_impl.rs:30,184-187`, drops if full); a background `std::thread::spawn` consumes and writes to disk. **The channel boundary shape is owned `String`**; an owned conversion on the request task is unavoidable.

2. **Serialization cannot be moved to the consumer thread**: `AccessLogEntry<'a>` borrows from the request context throughout (`request_info`, `backend_context`, `stage_logs`, `ctx_map`, etc.; see entry.rs:26-62). Moving JSON-ification to the consumer thread is equivalent to first cloning all these fields over — more allocations, complicated lifetimes.

3. **Per-call cost is negligible**: a single entry is 1-5 KB; jemalloc size-class hit + a single String allocation; absolute cost is < microsecond level. On the same request path, serde traversal + plugin/route execution + upstream I/O are all at ms level; the alloc here is not measurable in profile.

4. **"Save alloc" approaches do not actually save**:
   - thread-local `Vec<u8>` + `serde_json::to_writer`: still need `String::from(buf)` to put into the channel; alloc count unchanged.
   - `Bytes` / `Arc<str>`: the `DataSender<String>` trait + `LocalFileWriter::SyncSender<String>` + every logger (access / mirror / debug / tcp / udp) all need changes; alloc not saved.
   - Defer to the consumer thread: must clone all borrowed fields; not worthwhile.

5. **Real bottleneck is downstream**: channel full means drop; disk I/O is the ceiling. Halving `to_json()` does not change throughput.

**Verdict**: not a performance issue. Same category as "OIDC/JWT claims JSON serialization is the opt-in feature itself" — once access log is enabled, **JSON serialization is the feature contract**; channel-boundary owned String is determined by API shape. Any "access log `to_json()` should use a buffer pool / switch to `Bytes` / defer to consumer thread" finding is closed per this entry, unless it is proved that (a) per-entry size is ≥ tens of KB at high QPS, or (b) profile measurement shows `to_json` alloc has significant share on the request path.
