#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK_SCRIPT="${SCRIPT_DIR}/../check_bash_set_e_increment.sh"
TEMP_DIR="$(mktemp -d)"

cleanup() {
    rm -rf -- "$TEMP_DIR"
}
trap cleanup EXIT INT TERM

mkdir -p "${TEMP_DIR}/clean" "${TEMP_DIR}/bad" "${TEMP_DIR}/no-errexit"

cat > "${TEMP_DIR}/clean/example.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# A comment may mention ((count++)) without being executable.
echo "A quoted ((count++)) example is also harmless"
value=$((count++))
count=$((count + 1))
EOF

cat > "${TEMP_DIR}/bad/example.sh" <<'EOF'
#!/usr/bin/env bash
set -e
((count++))
((++other))
((remaining--))
((--last))
EOF

cat > "${TEMP_DIR}/no-errexit/example.sh" <<'EOF'
#!/usr/bin/env bash
((count++))
EOF

"$CHECK_SCRIPT" "${TEMP_DIR}/clean" >/dev/null
"$CHECK_SCRIPT" "${TEMP_DIR}/no-errexit" >/dev/null

bad_output="${TEMP_DIR}/bad-output.log"
if "$CHECK_SCRIPT" "${TEMP_DIR}/bad" >"$bad_output" 2>&1; then
    echo "ERROR: guard accepted unsafe arithmetic commands" >&2
    exit 1
fi

match_count="$(grep -c 'bare arithmetic increment/decrement under set -e' "$bad_output" || true)"
if [ "$match_count" -ne 4 ]; then
    echo "ERROR: expected 4 violations, found $match_count" >&2
    cat "$bad_output" >&2
    exit 1
fi

echo "OK: bash set-e increment guard regression tests passed"
