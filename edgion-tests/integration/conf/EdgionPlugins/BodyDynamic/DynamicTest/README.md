# Request Body Hot-Reload Test

This scenario verifies that a body-mutating plugin can be replaced online and
that subsequent requests use the new body transformation without restarting
the Gateway.

The initial phase replaces the JSON `version` field with `initial`. The update
phase replaces it with the longer `updated-body-version-is-longer` value. Both
phases also remove `discard`, so the assertions cover the complete committed
body rather than only one field.
