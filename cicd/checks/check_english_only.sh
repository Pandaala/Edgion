#!/usr/bin/env bash
# Fail if any non-English letter characters appear in the repository, except
# under docs/ and tasks/.
#
# - docs/ is the human-facing documentation layer (bilingual zh-CN / en).
# - tasks/ contains personal in-progress work notes.
# - Everything else (skills/, src/, examples/, cicd/, root files,
#   config/, etc.) must be English only.
#
# Blocked Unicode ranges (letter-bearing scripts of non-English languages):
#   U+0400-U+04FF  Cyrillic
#   U+0590-U+05FF  Hebrew
#   U+0600-U+06FF  Arabic
#   U+0900-U+097F  Devanagari
#   U+0E00-U+0E7F  Thai
#   U+1100-U+11FF  Hangul Jamo
#   U+3040-U+309F  Hiragana
#   U+30A0-U+30FF  Katakana
#   U+3400-U+4DBF  CJK Extension A
#   U+4E00-U+9FFF  CJK Unified Ideographs
#   U+AC00-U+D7AF  Hangul Syllables
#   U+F900-U+FAFF  CJK Compatibility Ideographs
#
# Punctuation / symbols (em-dash, smart quotes, arrows, bullets, accented
# Latin like é/ñ/ü, Greek letters like π) are NOT blocked — only letters
# from non-English writing systems are.
#
# Exit 0 on clean tree; exit 1 with offending matches on stderr otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

PATTERN='[\x{0400}-\x{04FF}\x{0590}-\x{05FF}\x{0600}-\x{06FF}\x{0900}-\x{097F}\x{0E00}-\x{0E7F}\x{1100}-\x{11FF}\x{3040}-\x{309F}\x{30A0}-\x{30FF}\x{3400}-\x{4DBF}\x{4E00}-\x{9FFF}\x{AC00}-\x{D7AF}\x{F900}-\x{FAFF}]'

if matches=$(rg -nP "${PATTERN}" \
    --glob '!docs/**' \
    --glob '!tasks/**' \
    . 2>/dev/null); then
    echo "ERROR: Non-English letter characters found outside docs/ and tasks/" >&2
    echo "${matches}" >&2
    exit 1
fi

echo "OK: no non-English letter scripts found outside docs/ and tasks/"
