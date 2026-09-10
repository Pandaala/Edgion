# External Sync — b1bea189

- Range: `b93fd0f3..b1bea189` (`d43d2539`, `b19975ed`, `b1bea189`), attributed to the user's concurrent task repairs.
- Gateway impact: eleven modified files, no additions or removals. Six terminal rows were reopened; five already-pending rows retained their historical evidence.
- Frozen integration impact: one modified file,
  `edgion-tests/src/client/dynamic/scenarios/request_mirror_reference_grant.rs`.
  Old Git object `2a558460` matched manifest SHA-256 `4d675cae...`; new
  Git object `2a2e786d` and worktree matched SHA-256 `9f68f66a...`, mode
  remained non-executable. Only this manifest entry was updated.
- D0100 was completed by `d43d2539`; D0144 was completed by `b1bea189`.
  Their standalone tasks were removed externally and were not recreated.
- B0289 target stayed byte-identical. Its analyst and independent reviewer
  revalidated the changed body-filter/adapter/snapshot consumers at the new HEAD.
  No campaign integration file was edited.
