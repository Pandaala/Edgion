# A2A Gateway Hot-Reload Test

This scenario publishes an operator-owned local Agent Card, updates its version
and interface path through the existing EdgionPlugins partial-update path, and
polls the client-visible card until both values come from the new immutable
runtime snapshot. The Gateway and Controller are not restarted.

Each phase also sends a fresh JSON-RPC request through an A2A backend and
requires exactly one bounded response summary. This proves new requests use
the observer installed by the current immutable plugin snapshot after reload;
the scenario deliberately makes no claim about replacing an in-flight stream.
