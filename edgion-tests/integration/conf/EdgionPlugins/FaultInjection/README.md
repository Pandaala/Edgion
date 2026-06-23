# FaultInjection Integration Tests

## Overview
Tests delay/abort injection at the gateway request_filter stage.

## Routing
Host: `fault.example.com`, prefix `/test/fault/<scenario>`. Gateway port `31180`.

## Scenarios
| File | EdgionPlugins | Path prefix | What it exercises |
|---|---|---|---|
| 01 | fault-delay-fixed | /test/fault/delay-fixed | fixed 300ms delay, status from upstream |
| 02 | fault-delay-range | /test/fault/delay-range | random delay in [100,500]ms |
| 03 | fault-abort | /test/fault/abort | 503 with body + custom header |
| 04 | fault-delay-and-abort | /test/fault/delay-and-abort | delay then abort |
| 05 | fault-ratio-zero | /test/fault/ratio-zero | both ratios=0 -> pass-through |
| 06 | fault-entry-conditions | /test/fault/entry-conditions | entry `conditions.run` gates by header |
| 07 | fault-abort-empty | /test/fault/abort-empty | 204 + empty body |
