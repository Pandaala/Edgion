#!/usr/bin/env bash
# Convenience forwarder → integration/scripts/integration/run_integration.sh
# The real runner resolves its own location via BASH_SOURCE, so exec'ing it from
# here is fully transparent: all args pass through unchanged.
exec "$(dirname "${BASH_SOURCE[0]}")/integration/scripts/integration/run_integration.sh" "$@"
