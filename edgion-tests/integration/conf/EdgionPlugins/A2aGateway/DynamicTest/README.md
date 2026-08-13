# A2A Gateway Hot-Reload Test

This scenario publishes an operator-owned local Agent Card, updates its version
and interface path through the existing EdgionPlugins partial-update path, and
polls the client-visible card until both values come from the new immutable
runtime snapshot. The Gateway and Controller are not restarted.
