#!/usr/bin/env python3
"""Guard: every server-side-apply (`Patch::Apply`) must use forced `PatchParams`.

WHY THIS EXISTS
---------------
Kubernetes server-side apply only overwrites fields the *same* field manager
already owns. Any other owner makes the API server return 409 Conflict instead
of applying. A controller that is the sole authority for an object's content
therefore MUST pass `force`, or it wedges permanently the moment another writer
touches the object — and "another writer" is the normal case:

  * an object the controller CREATEs with a POST has its fields recorded under
    the client's default manager (`unknown`), NOT the apply field manager;
  * an operator `kubectl create/edit/patch` hands field ownership to kubectl;
  * a Secret pre-created before adopting the controller (a documented migration
    path for both ACME TLS Secrets and the conf_sync CA) is owned by whoever
    created it.

This was not hypothetical: an unforced apply in `conf_sync_ca::secret_io` made
CA renewal 409 forever (root expires -> conf_sync outage), found only in cluster
verification because unit tests and local file-mode e2e never see the API
server's ownership bookkeeping. Forcing is the correct semantic here — SSA force
takes ownership of exactly the fields the apply writes, leaving operator edits to
other fields intact (the standard controller-runtime recommendation).

`force` is honored ONLY for `Patch::Apply`; it is ignored for `Patch::Merge` /
`Patch::Json` / `Patch::Strategic`, which have no field-ownership conflicts. Such
call sites legitimately build `PatchParams::apply(...)` just to set the field
manager (kube exposes no other setter) and live in ALLOWLIST below.

KNOWN LIMITS (this is a lint, not a type system)
------------------------------------------------
It keys on the literal `PatchParams::apply(` construction and requires `.force()`
within the same statement. It therefore does NOT catch:
  * a `PatchParams` built via struct literal (`PatchParams { force: false, .. }`)
    or mutated after construction (`pp.force = false;`);
  * params constructed in one function and passed into another.
Both forms are absent from the tree today; if one appears, extend this check
rather than routing around it. It also skips only FULL-LINE `//` comments, so a
trailing comment that merely names `PatchParams::apply(` would false-positive —
move such prose to its own line.
"""

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CRATE_GLOBS = ["edgion-*/src/**/*.rs"]

# (relative_path, enclosing_fn) -> reason. Keyed by function, not line number, so
# the allowlist survives unrelated edits above it.
ALLOWLIST = {
    (
        "edgion-controller/src/services/acme/secret_io.rs",
        "patch_account_key_into_existing_secret",
    ): (
        "Patch::Merge, not Patch::Apply — `force` is only honored for apply "
        "patches, and merge patches raise no field-ownership conflict. "
        "PatchParams::apply() is used here solely to set the field manager "
        "(kube exposes no other setter for it)."
    ),
}

APPLY_CALL = re.compile(r"PatchParams::apply\s*\(")
FN_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)")
# Line comments (`//`, `///`, `//!`) — prose that merely *names* the API is not a
# call site. Block comments are not used for this API anywhere in the tree.
COMMENT_LINE = re.compile(r"^\s*//")


def enclosing_fn(lines, idx):
    """Nearest `fn NAME` at or above line index `idx`."""
    for i in range(idx, -1, -1):
        m = FN_DECL.match(lines[i])
        if m:
            return m.group(1)
    return "<unknown>"


def statement_at(lines, idx):
    """The statement starting at `idx`, joined until its terminating `;`.

    Covers the builder-chain-across-lines form:
        let pp = PatchParams::apply(FIELD_MANAGER)
            .force();
    """
    chunk = []
    for i in range(idx, min(idx + 8, len(lines))):
        chunk.append(lines[i])
        if ";" in lines[i]:
            break
    return " ".join(chunk)


def main():
    violations = []
    used_allowlist = set()

    files = sorted(
        {p for g in CRATE_GLOBS for p in REPO_ROOT.glob(g)},
        key=lambda p: str(p),
    )
    for path in files:
        rel = str(path.relative_to(REPO_ROOT))
        lines = path.read_text(encoding="utf-8").splitlines()
        for idx, line in enumerate(lines):
            if COMMENT_LINE.match(line) or not APPLY_CALL.search(line):
                continue
            fn = enclosing_fn(lines, idx)
            key = (rel, fn)
            stmt = statement_at(lines, idx)
            if ".force()" in stmt:
                if key in ALLOWLIST:
                    violations.append(
                        f"{rel}:{idx + 1} (fn `{fn}`): allowlisted as non-apply, but the "
                        f"statement DOES call .force() — remove the stale ALLOWLIST entry."
                    )
                continue
            if key in ALLOWLIST:
                used_allowlist.add(key)
                continue
            violations.append(
                f"{rel}:{idx + 1} (fn `{fn}`): PatchParams::apply(...) without .force()"
            )

    for key in ALLOWLIST:
        if key not in used_allowlist:
            violations.append(
                f"stale ALLOWLIST entry {key[0]} (fn `{key[1]}`): no matching "
                f"unforced PatchParams::apply(...) found — delete it."
            )

    if violations:
        print("SSA force guard FAILED:\n", file=sys.stderr)
        for v in violations:
            print(f"  - {v}", file=sys.stderr)
        print(
            "\nAn unforced `Patch::Apply` 409-Conflicts forever as soon as any other\n"
            "field manager owns the object (a POST-created object, a `kubectl edit`,\n"
            "or a Secret that predates the controller). Fix by chaining `.force()`:\n"
            "    PatchParams::apply(FIELD_MANAGER).force()\n"
            "If the call site uses Patch::Merge/Json/Strategic (where `force` is\n"
            "ignored and no ownership conflict exists), add it to ALLOWLIST in\n"
            "cicd/checks/check_ssa_force.py with a reason.\n",
            file=sys.stderr,
        )
        return 1

    print(f"SSA force guard passed: checked {len(files)} files, {len(ALLOWLIST)} allowlisted.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
