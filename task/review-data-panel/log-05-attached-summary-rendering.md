# Attached ExtProc summary rendering

Status: deferred summary interface and rendering decision for the user.
Source baseline: 8a3a4ceb4. No runtime change.

## Evidence

`edgion-gateway/src/plugins/http/ext_proc/attached.rs::take_summary_line`
claims `emitted` and copies phases, input bytes, output bytes, outcome and reason
under the summary mutex. Numeric and static-text fields are then formatted into
an owned String after releasing the lock. The earlier lock-shortening change
already removed formatting from the critical section; this is a separate
candidate concerning intermediate storage.

Its sole production consumer,
`routes/http/proxy_http/pg_logging.rs::finalize_attached_ext_proc_logs`, first
terminalizes the owner, claims the line, scans stage logs for its internal token,
and copies the line into the first matching PluginLog. The summary is claimed
even if no log matches or the bounded buffer rejects the entry. Later owner
updates cannot change the captured line.

The existing canonical `PluginLog::push_fmt` can render typed primitive integers
and static plain text using 128 bytes of inline scratch before copying into the
same bounded log buffers. Larger entries may spill to the heap. This creates a
conditional opportunity to remove the temporary String; it does not establish
zero allocation for long summaries, overflow logging, final serialization or the
whole request. No new renderer or log destination is needed.

## Alternatives and decision

- Keep the existing owned-line interface until the change is approved.
- Return a small immutable typed summary from the same single-claim operation,
  then render it through the canonical PluginLog formatter after finding its
  destination. Decide whether the snapshot owns its rendering method or the
  terminal logger does. Keep one output format and one summary owner.

A direct switch to formatting only after finding a log must not move the claim
itself after the search. Likewise, borrowing the mutable summary until insertion
would lose the frozen-output contract. The interface change should be reviewed
across the attached owner, terminal logger and existing tests rather than
introduced as an isolated String deletion.

## Acceptance for an approved implementation

Preserve exact field names, order, spacing, decimal rendering and plain-text
classification; outcome and reason remain bounded static diagnostics, never
external status text or secrets. Keep repeated-claim refusal and the same frozen
values after later updates. Preserve claim on absent destination, first-match
placement across native filters and resource logs, and buffer overflow/order/
log_full behavior. Do not reclassify the established aggregate summary or add
per-phase tracing, metrics or duplicate access-log entries.

Compare serialized logs and acceptance results with the existing path for zero
and maximum counters, all terminal outcomes, empty and populated tier-one/tier-two
buffers, unmatched and duplicate internal tokens, and rejected writes. Measure
short and spilled summary rendering separately; include the storage already
allocated by the caller when interpreting allocation results. Preserve terminal
cleanup, prior outcomes and exactly-once owner finalization.

Read `skills/03-coding/observability/00-access-log.md`,
`skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md`
and the current PluginLog formatter/buffer implementations before freezing scope.
This task authorizes neither an implementation nor a new logging abstraction.

## Current verification

Fresh baseline passed 143 ExtProc and ten terminal-logging tests (153 total),
formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated Clippy warnings remain. These checks cover the current owned-line
path, not the proposed snapshot API. No full suite, live integration or
throughput benchmark ran.
