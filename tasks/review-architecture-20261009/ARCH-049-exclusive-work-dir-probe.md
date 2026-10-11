---
id: ARCH-049
status: candidate
kind: behavior-defect
priority: P2
partition: PART-187
analyst: /root/analyze_outbound
independent_verification: confirmed by /root/verify_ip_union
---

# Make the startup writability probe own its file exclusively

WorkDir::validate writes four bytes to the fixed child .edgion_write_test and
then removes that path. std::fs::write creates or truncates and follows existing
symbolic links. Validation never establishes ownership of the overwritten or
removed object. An existing writable regular file at that name is truncated
and deleted. A link to another writable file causes target truncation and link
removal. Canonicalizing the base directory does not protect this child path.
Later validation failure cannot restore the data.

Both Gateway CLI startup and Controller init_environment select and validate
the work directory before logging initialization. The default is the current
directory; configured directories can contain existing files. No reserved-name
ownership check declares that child disposable. These are conditional source
witnesses, not an observed incident, privilege-escalation claim or executed
filesystem reproduction.

Create a private temporary file exclusively inside the selected directory,
using OpenOptions::create_new(true) and bounded unique-name collision handling,
or a suitable owned temporary-file facility. Write the existing four probe
bytes through the returned handle. Retry only creation collisions within the
bound, propagate other create/write errors through the existing startup error
contract, and attempt cleanup after a successful creation on both write success
and failure. Never run cleanup for a failed creation or a collided pathname.
Keep cleanup failure best-effort with the existing startup warning policy;
current cleanup failure does not by itself fail validation. A small local
ownership guard or explicit error-path cleanup is sufficient.

This remedy prevents overwriting or unlinking objects already occupying a probe
candidate at creation time. Exclusive creation alone does not prevent another
directory writer from later replacing the pathname before path-based cleanup,
and this finding makes no adversarial post-creation pathname guarantee. A broader
hostile-directory threat model or platform-specific race-proof unlink design is
outside this bounded correction. Do not claim such protection from a unique
name, an existence check, or an ownership guard alone.

A standard-library solution avoids promoting test-only tempfile or introducing
a general factory. A fixed-name create_new probe prevents the existing data-loss
witness but can reject a writable directory for an unrelated filename; bounded
unique-name attempts avoid that unnecessary fixed-name dependence. Checking
existence before write still races and establishes no exclusive creation.

Keep a real create-and-write check rather than permission-bit inspection, which
cannot prove writability under ACLs, mount policy and process credentials.
Preserve CLI/environment/config/default precedence, directory layout, directory
creation, path resolution and binary-owned logging. Add no request-path check,
traffic tracing or metrics. The work-directory guide's unsafe example describes
current code, not a requirement to overwrite a fixed pathname; update the
relevant example only during a later authorized implementation.

Existing tests inspect empty directories, creation, readonly directories and
symlinked bases. Future verification should preserve a pre-existing
.edgion_write_test file's exact bytes and pathname, include a Unix-gated
pre-existing child-symlink case retaining both link and target, and establish
cleanup of a successfully created probe. Cover failed-write cleanup where a
deterministic fixture is practical without a general production IO abstraction.
Keep existing directory/layout tests and acknowledge readonly-test privilege
limitations. The probe filename itself need not remain fixed.

Original applicable rules, actual startup consumers, complete work-directory
implementation/test bodies and the original guide were personally read.
Static read-only verification only: no writes, tests, builds, network, Git
operations or filesystem/application reproduction. Full primary analysis
receipts belong to REPORT-PART-187.json; independent verification receives no
source primary coverage.
