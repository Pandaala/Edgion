#!/usr/bin/env python3
"""Plain-python self-test for check_metrics_inventory.py (no pytest).

Run with: cd cicd/checks && python3 test_check_metrics_inventory.py
"""
from pathlib import Path

from check_metrics_inventory import (
    diff,
    extract_metric_names_from_files,
    parse_inventory,
    scan_dangerous_labels,
)

HERE = Path(__file__).parent / "testdata" / "metrics_inventory"

EXPECTED_NAMES = {
    "edgion_sample_a_total",
    "edgion_sample_b",
    "edgion_sample_c",
}


def test_extract_all_three_forms():
    names = extract_metric_names_from_files([HERE / "sample.rs"])
    assert names == EXPECTED_NAMES, names


def test_parse_inventory():
    names = parse_inventory(HERE / "inventory_ok.md")
    assert names == EXPECTED_NAMES, names


def test_no_drift_ok():
    code = extract_metric_names_from_files([HERE / "sample.rs"])
    inv = parse_inventory(HERE / "inventory_ok.md")
    assert diff(code, inv) == ([], [])


def test_comments_are_ignored():
    fixture = """
    // const FAKE_A: &str = "edgion_commented_a";
    /* metrics::counter!("edgion_commented_b") */
    const REAL: &str = "edgion_real_one";
    fn emit() {
        metrics::gauge!("edgion_real_two").set(1.0);
    }
    """
    tmp = HERE / "_tmp_comments.rs"
    tmp.write_text(fixture)
    try:
        names = extract_metric_names_from_files([tmp])
    finally:
        tmp.unlink()
    assert names == {"edgion_real_one", "edgion_real_two"}, names


def test_extract_static_lifetime_const():
    # A metric-name const written with an explicit `&'static str` (or `&'a str`)
    # lifetime must still be extracted — otherwise it would silently escape the
    # drift guard and could be undocumented while the guard still passes.
    fixture = """
    pub const A: &str = "edgion_plain_total";
    pub const B: &'static str = "edgion_static_total";
    const C: &'a str = "edgion_named_lt_total";
    """
    tmp = HERE / "_tmp_static_lt.rs"
    tmp.write_text(fixture)
    try:
        names = extract_metric_names_from_files([tmp])
    finally:
        tmp.unlink()
    assert names == {
        "edgion_plain_total",
        "edgion_static_total",
        "edgion_named_lt_total",
    }, names


def test_line_comment_with_stray_block_open():
    # A `//` line comment that itself mentions `/*` must not be misread as
    # opening a real block comment that then swallows the next real code up
    # to the next genuine `*/` (here, the closing `*/` of the real block
    # comment further down that genuinely wraps FAKE).
    fixture = """
    // this is a line comment mentioning /* fake open
    const REAL: &str = "edgion_should_not_be_removed";
    metrics::counter!("edgion_should_not_be_removed_either").increment(1);
    /* actual comment
    const FAKE: &str = "edgion_should_be_removed"; */
    """
    tmp = HERE / "_tmp_stray_block_open.rs"
    tmp.write_text(fixture)
    try:
        names = extract_metric_names_from_files([tmp])
    finally:
        tmp.unlink()
    assert "edgion_should_not_be_removed" in names, names
    assert "edgion_should_not_be_removed_either" in names, names
    assert "edgion_should_be_removed" not in names, names


def test_dangerous_label_detected():
    hits = scan_dangerous_labels([HERE / "danger.rs"])
    assert ("edgion_bad_total", "path") in hits, hits
    assert all(key != "status" for _name, key in hits), hits


def test_dangerous_label_real_form():
    # Real emit sites have no `metrics::` prefix and use a `names::` const
    # (not a string literal) as the first argument — the form this check
    # historically missed entirely.
    fixture = """
    fn emit() {
        counter!(names::FOO, "path" => p).increment(1);
        counter!(names::BAR, "status" => s).increment(1);
    }
    """
    tmp = HERE / "_tmp_real_form.rs"
    tmp.write_text(fixture)
    try:
        hits = scan_dangerous_labels([tmp])
    finally:
        tmp.unlink()
    assert any(key == "path" for _name, key in hits), hits
    assert all(key != "status" for _name, key in hits), hits


def test_dangerous_label_after_nested_call():
    # A nested call inside an earlier label value (`normalize(p)`) must not
    # confuse the paren-balance tracking and truncate the argument list
    # before the later `"path"` label is seen.
    fixture = """
    fn emit() {
        counter!(names::X, "user" => normalize(p), "path" => p).increment(1);
    }
    """
    tmp = HERE / "_tmp_nested_call.rs"
    tmp.write_text(fixture)
    try:
        hits = scan_dangerous_labels([tmp])
    finally:
        tmp.unlink()
    assert any(key == "path" for _name, key in hits), hits


if __name__ == "__main__":
    import sys

    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    failed = 0
    for t in tests:
        try:
            t()
            print(f"PASS: {t.__name__}")
        except Exception as e:
            failed += 1
            print(f"FAIL: {t.__name__}: {e!r}")
    print(f"{len(tests)-failed} passed, {failed} failed")
    sys.exit(1 if failed else 0)
