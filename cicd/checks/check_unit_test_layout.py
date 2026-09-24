#!/usr/bin/env python3
"""Check the migrated edgion-common unit-test mounts.

This deliberately scopes enforcement to the completed pilot crate. Expand the
scope as other crates migrate. Cargo compilation and test-identity comparisons
remain necessary for macro expansions, cfg combinations, and privacy checks.
"""

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "edgion-common"
CFG = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
PATH = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]')
MODULE = re.compile(r"mod\s+([A-Za-z_][A-Za-z_0-9]*)\s*([;{])")
TEST = re.compile(r"#\s*\[\s*(?:(?:tokio|async_std)\s*::\s*)?test\b")
REEXPORT = re.compile(r"pub\s*\(\s*crate\s*\)\s+use\s+test_support::outbound_tls_test_guard\s*;")


def mask_non_code(text: str) -> str:
    """Keep offsets/newlines while removing Rust comments and string contents."""
    out = list(text)
    i = 0
    n = len(text)

    def blank(start: int, end: int) -> None:
        for k in range(start, end):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
        elif text.startswith("/*", i):
            depth = 1
            end = i + 2
            while end < n and depth:
                if text.startswith("/*", end):
                    depth += 1
                    end += 2
                elif text.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank(i, end)
            i = end
        else:
            raw = re.match(r'(?:b|br|r)?(#+)?"', text[i:])
            if raw:
                hashes = raw.group(1) or ""
                end_marker = '"' + hashes
                start = i + raw.end()
                end = text.find(end_marker, start)
                end = n if end < 0 else end + len(end_marker)
                if not hashes and text[i] not in "r":
                    # Ordinary strings can contain escaped quotes.
                    end = start
                    while end < n:
                        if text[end] == "\\":
                            end += 2
                        elif text[end] == '"':
                            end += 1
                            break
                        else:
                            end += 1
                blank(i, end)
                i = end
            else:
                i += 1
    return "".join(out)


def check() -> list[str]:
    errors = []
    mounts = {}
    support = (CRATE / "tests" / "support" / "outbound_tls.rs").resolve()
    for source in sorted((CRATE / "src").rglob("*.rs")):
        original = source.read_text()
        code = mask_non_code(original)
        relative = source.relative_to(CRATE / "src")
        for match in CFG.finditer(code):
            cursor = match.end()
            while cursor < len(code) and code[cursor].isspace():
                cursor += 1
            path_match = PATH.match(original, cursor)
            if path_match:
                cursor = path_match.end()
                while cursor < len(code) and code[cursor].isspace():
                    cursor += 1
            module = MODULE.match(code, cursor)
            if not module:
                if source.relative_to(CRATE / "src") == Path("link_sys_core/outbound_tls.rs") and REEXPORT.match(code, cursor):
                    continue
                line = original.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: test-only implementation remains in src")
                continue
            name, kind = module.groups()
            line = original.count("\n", 0, match.start()) + 1
            if kind != ";":
                errors.append(f"{relative}:{line}: inline test module {name}")
                continue
            if not path_match:
                errors.append(f"{relative}:{line}: missing #[path] for {name}")
                continue
            actual = (source.parent / path_match.group(1)).resolve()
            base = CRATE / "tests" / "unit" / relative.parent / f"{relative.stem}_test"
            if relative == Path("link_sys_core/outbound_tls.rs") and name == "test_support":
                expected = support
            else:
                expected = (base.with_suffix(".rs") if name == "tests" else base / f"{name}.rs").resolve()
            if actual != expected:
                errors.append(f"{relative}:{line}: {name} mounts {actual}, expected {expected}")
            if not actual.is_file():
                errors.append(f"{relative}:{line}: missing {actual}")
            mounts.setdefault(actual, []).append(f"{relative}:{line}")
        for match in TEST.finditer(code):
            line = original.count("\n", 0, match.start()) + 1
            errors.append(f"{relative}:{line}: test attribute remains in src")
    for path, owners in mounts.items():
        if len(owners) != 1:
            errors.append(f"{path}: duplicate mounts: {', '.join(owners)}")
    for path in sorted((CRATE / "tests" / "unit").rglob("*.rs")):
        if path.resolve() not in mounts:
            errors.append(f"{path.relative_to(CRATE)}: orphan unit-test file")
    if support.is_file() and support not in mounts:
        errors.append(f"{support.relative_to(CRATE)}: orphan support file")
    return errors


if __name__ == "__main__":
    failures = check()
    if failures:
        print("\n".join(failures), file=sys.stderr)
        sys.exit(1)
    print(f"Unit-test layout passed: {len(list((CRATE / 'tests' / 'unit').rglob('*.rs')))} mounted files")
