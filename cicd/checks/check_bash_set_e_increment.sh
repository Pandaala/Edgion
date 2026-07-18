#!/usr/bin/env bash
# Reject standalone arithmetic increment/decrement commands in integration
# scripts that enable errexit. When the expression evaluates to zero, the
# arithmetic command returns status 1 and can terminate the script silently.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

if [ "$#" -gt 0 ]; then
    SCAN_PATHS=("$@")
else
    SCAN_PATHS=("${REPO_ROOT}/edgion-tests/integration/scripts")
fi

SCRIPTS=()
for scan_path in "${SCAN_PATHS[@]}"; do
    if [ -d "$scan_path" ]; then
        while IFS= read -r script; do
            SCRIPTS+=("$script")
        done < <(find "$scan_path" -type f -name '*.sh' -print)
    elif [ -f "$scan_path" ]; then
        SCRIPTS+=("$scan_path")
    else
        echo "ERROR: bash portability scan path does not exist: $scan_path" >&2
        exit 2
    fi
done

violations=0
for script in "${SCRIPTS[@]}"; do
    if ! grep -Eq '^[[:space:]]*set[[:space:]]+-[^#]*e' "$script"; then
        continue
    fi

    hits="$(awk '
        function executable_code(line,    out, i, ch, quote, escaped) {
            out = ""
            quote = ""
            escaped = 0
            for (i = 1; i <= length(line); i++) {
                ch = substr(line, i, 1)
                if (escaped) {
                    out = out " "
                    escaped = 0
                    continue
                }
                if (ch == "\\") {
                    out = out " "
                    escaped = 1
                    continue
                }
                if (quote != "") {
                    out = out " "
                    if (ch == quote) {
                        quote = ""
                    }
                    continue
                }
                if (ch == "\047" || ch == "\042") {
                    quote = ch
                    out = out " "
                    continue
                }
                if (ch == "#") {
                    break
                }
                out = out ch
            }
            return out
        }

        BEGIN {
            pattern = "[(][(][[:space:]]*((([+][+]|--)[[:space:]]*[A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*[[:space:]]*([+][+]|--)))[[:space:]]*[)][)]"
        }

        {
            code = executable_code($0)
            remainder = code
            offset = 0
            while (match(remainder, pattern)) {
                start = offset + RSTART
                previous = start > 1 ? substr(code, start - 1, 1) : ""
                if (previous != "$") {
                    print FILENAME ":" FNR ": bare arithmetic increment/decrement under set -e -- use an assignment such as x=$((x + 1))"
                }
                offset += RSTART + RLENGTH - 1
                remainder = substr(remainder, RSTART + RLENGTH)
            }
        }
    ' "$script")"

    if [ -n "$hits" ]; then
        echo "$hits" >&2
        violations=$((violations + 1))
    fi
done

if [ "$violations" -gt 0 ]; then
    echo "ERROR: found bare arithmetic increment/decrement commands in $violations set -e script(s)" >&2
    exit 1
fi

echo "OK: no bare arithmetic increment/decrement commands found in set -e integration scripts"
