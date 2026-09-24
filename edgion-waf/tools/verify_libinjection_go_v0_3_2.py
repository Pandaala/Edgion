#!/usr/bin/env python3
"""Verify vendored detector data and offline fixtures against pinned Go source.

Usage: python3 edgion-waf/tools/verify_libinjection_go_v0_3_2.py /path/to/libinjection-go
"""

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

COMMIT = "f827564b9f5bf446a038deabd535361e114109ce"
SOURCE_SHA256 = {
    "LICENSE": "3296b49ab7f460064a220ce0c5b0ea698a9c82a3d72bec5f10d0e0529cce1023",
    "sqli_data.go": "9ce331e1c82bf1efb570c2f4963ac8ff6272b09007399bbc623f7b8f92cff9a2",
    "sqli_const.go": "e52d54e8311979064968b7573e90acc6bc263c9a223c73a5626319f37f9dcfae",
    "xss_decls.go": "bb2916332233136e8d51a7ded29def3eb4b0b6c3a99327302566256b5d6de781",
    "xss_helpers.go": "b3b5c9b21fc8b3b2d9ee9f0cf3d3177bd1a29aefef15d76d8dc38851c0d46b91",
    "html5_decls.go": "632eea3b8da94f2a6f3bed34bb80a0162090400a207bb590164ec3585f1ef62d",
}
ROOT = Path(__file__).resolve().parents[1]


def section(text: str, start: str, end: str) -> str:
    return text.split(start, 1)[1].split(end, 1)[0]


def ascii_fixtures(upstream: Path) -> tuple[list[dict[str, str]], list[str]]:
    result = []
    excluded = []
    for file in sorted((upstream / "tests").iterdir()):
        raw = file.read_bytes()
        value = raw.split(b"--INPUT--\n", 1)[1]
        input_raw, expected_raw = value.split(b"--EXPECTED--\n", 1)
        input_raw = input_raw.rstrip(b" \t\r\n")
        expected_raw = expected_raw.rstrip(b" \t\r\n")
        if not input_raw.isascii() or not expected_raw.isascii():
            excluded.append(file.name)
            continue
        result.append({
            "source": file.name,
            "input": input_raw.decode("ascii"),
            "expected": expected_raw.decode("ascii"),
        })
    return result, excluded


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    upstream = Path(sys.argv[1]).resolve()
    head = subprocess.check_output(["git", "-C", str(upstream), "rev-parse", "HEAD"], text=True).strip()
    assert head == COMMIT, f"expected {COMMIT}, got {head}"
    for name, digest in SOURCE_SHA256.items():
        actual = hashlib.sha256((upstream / name).read_bytes()).hexdigest()
        assert actual == digest, f"upstream {name}: {actual}"
    assert (ROOT / "licenses/libinjection-go-BSD-3-Clause.txt").read_bytes() == (upstream / "LICENSE").read_bytes()

    go_sql = section((upstream / "sqli_data.go").read_text(), "var sqlKeywords = map[string]byte{", "\n}")
    rust_sql = section((ROOT / "src/sqli/generated_data.rs").read_text(), "pub const SQL_KEYWORDS: &[Keyword] = &[", "\n];")
    go_keywords = dict(re.findall(r'^\s*"([^"]+)":\s*\'(.)\',', go_sql, re.M))
    rust_keywords = dict(re.findall(r'Keyword \{ word: "([^"]+)", token_type: b\'(.)\' \}', rust_sql))
    assert len(go_keywords) == len(rust_keywords) == 9352
    assert go_keywords == rust_keywords, "SQL keyword or fingerprint table differs"

    go_xss = (upstream / "xss_decls.go").read_text()
    rust_xss = (ROOT / "src/xss/blacklists.rs").read_text()
    for go_name, rust_name, next_go, next_rust in [
        ("blackEvents", "BLACK_ATTR_EVENTS", "var blackTags", "pub const BLACK_ATTRS"),
        ("blacks", "BLACK_ATTRS", "var gsHexDecodeMap", "pub const BLACK_TAGS"),
    ]:
        go_part = section(go_xss, f"var {go_name} = []stringType{{", next_go)
        rust_part = section(rust_xss, f"pub const {rust_name}: &[StringType] = &[", next_rust)
        go_items = dict(re.findall(r'\{"([^"]+)", attributeType(\w+)\}', go_part))
        rust_items = dict(re.findall(r'StringType\s*\{\s*name: "([^"]+)",\s*atype: AttributeType::(\w+),?\s*\}', rust_part))
        normalized = {key: value.replace("AttrURL", "AttrUrl") for key, value in go_items.items()}
        assert normalized == rust_items, f"XSS {go_name} differs"
        if go_name == "blackEvents":
            assert len(go_items) == 432
    go_tags = re.findall(r'^\s*"([A-Z]+)",', section(go_xss, "var blackTags = []string{", "\n}"), re.M)
    rust_tags = re.findall(r'"([A-Z]+)"', section(rust_xss, "pub const BLACK_TAGS: &[&str] = &[", "\n];"))
    assert go_tags == rust_tags
    go_hex = [int(value) for value in re.findall(r"\b\d+\b", section(go_xss, "var gsHexDecodeMap = []int{", "\n}"))]
    rust_hex = [int(value) for value in re.findall(r"\b\d+\b", section(rust_xss, "pub const HEX_DECODE_MAP: [i32; 256] = [", "\n];"))]
    assert len(go_hex) == len(rust_hex) == 256 and go_hex == rust_hex
    protocols = re.findall(
        r'^\s*"([A-Z-]+)",',
        section((upstream / "xss_helpers.go").read_text(), '"DATA",', "\n\t}"),
        re.M,
    )
    rust_protocols = re.findall(r'"([A-Z-]+)"', re.search(r'pub const BLACK_URL_PROTOCOLS: &\[&str\] = &\[(.*?)\];', rust_xss, re.S).group(1))
    assert ["DATA", *protocols] == rust_protocols

    expected, excluded = ascii_fixtures(upstream)
    assert len(expected) == 492 and len(excluded) == 4, (len(expected), excluded)
    corpus = ROOT / "tests/fixtures/libinjection-go-v0.3.2-parser.json"
    assert json.loads(corpus.read_text()) == expected, "offline parser fixtures differ"
    verdicts = json.loads((ROOT / "tests/fixtures/libinjection-go-v0.3.2.json").read_text())
    sources = {case["source"]: case for case in expected}
    assert len(verdicts) == 61
    assert len({case["source"] for case in verdicts}) == 61
    for verdict in verdicts:
        source = verdict["source"]
        fixture = sources[source]
        assert verdict["input"] == fixture["input"], source
        if "-sqli-" in source:
            assert verdict["fingerprint"] == fixture["expected"], source
            assert verdict["sqli"] == bool(fixture["expected"]), source
        else:
            assert verdict["xss"] == (fixture["expected"] == "1"), source
    print(f"verified Go {COMMIT}: 9352 SQL entries, 432 XSS events, 492 offline fixtures; 4 non-ASCII fixtures remain external")


if __name__ == "__main__":
    main()
