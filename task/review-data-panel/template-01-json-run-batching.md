# Template JSON escape run batching

Status: deferred for user decision. No runtime change.
Source baseline: 03947e0e8.

## Evidence and scope

The canonical `Output::append_json` in
`edgion-gateway/src/link_sys/template/engine.rs` walks Unicode scalar values and
calls `append` for each ordinary character or complete escape. Each append checks
aggregate length and optional output capacity before writing. Scanning contiguous
unescaped UTF-8 spans and appending each span once could reduce these repeated
checks and writes. Escaped characters still require their existing exact spelling.

Appending a span changes the sequence of String growth requests. A local isolated
writer probe used the current append policy, initial capacity 32, 70 ASCII `a`
characters, and the two-byte escaped quote. The scalar and span paths produced
identical 72-byte output, but final capacities were 128 and 140 respectively.
The result held with both `max_bytes: None` and `Some(256)` on this toolchain.
The span first grows to 70; the subsequent escape causes growth to 140. The scalar
path grows through 64 to 128. These are observed String capacities, not stable
allocator guarantees, physical heap usage, peak-memory or throughput results.

The parser uses a 32-byte variable hint plus literal lengths, and the renderer
starts from its aggregate hint (clamped to an optional cap). The isolated probe
is not a complete admitted JSON-template benchmark. Actual templates, escaped
inputs and initial hints need representative measurement before choosing a policy.

The production JSON escape consumer is `CompiledBody::Json` in the shared outbound
request builder. It selects JsonString with the existing uncapped EMPTY policy.
Bounded JSON rendering is a supported and tested capability of the same writer;
this candidate does not authorize adding a new operator limit or changing a
consumer's policy. Templates still escape substitutions only, never literals.

A span that exceeds the remaining cap can also leave a different private partial
prefix than per-character appends. The renderer returns an error and discards
that output, so this alone is not an externally visible behavior regression.
Preserve the public error and capability evaluation outcomes. The retained-capacity
tradeoff, not a proven wire or error regression, is the reason for deferral.

## Decision requested

Retain the current character walker, authorize span batching with its capacity
tradeoff, or investigate a growth strategy that preserves the desired storage
bounds while reducing append work. Simulating scalar growth may recover storage
behavior but adds arithmetic/scanning cost; measure it rather than assuming an
improvement. Do not introduce a consumer-local serializer or an alternate template
walker to bypass the canonical mechanism.

## Acceptance before implementation

Use IMPL with a frozen renderer/test/knowledge allow-list. Preserve exact escape
spelling for all controls, quotes and backslashes, allowed multibyte UTF-8,
literal-versus-variable handling, empty input, aggregate overflow and logical
byte caps. Maintain whole-escape admission, source-sensitive flags, Secret and
Request/Trusted capabilities, missing-value policies and owned final output.
Do not replace this code with a serializer whose control escape spelling differs.

Compare the old character walker and proposed implementation across ordinary runs,
interleaved escapes, all control bytes, multibyte boundaries, many substitutions,
initial capacities and exact/under/over caps. Keep existing geometric-growth guards
meaningful. Measure allocation calls, requested bytes and retained capacity apart
from CPU/throughput; include the real outbound JSON builder and its ownership
lifetime. Run template/outbound tests, relevant consumers and repository checks,
and obtain independent final review before implementing or claiming a gain.
