# Repository Agent Instructions

## Upstream integration

Before fetching, merging, cherry-picking, replaying, or reviewing changes from
the upstream Konnect repository, read and follow
[`docs/UPSTREAM_INTEGRATION.md`](docs/UPSTREAM_INTEGRATION.md).

Its stable constraints are mandatory. In particular:

- use the permanent `dev` branch for integration and keep `main` as the release
  line; do not create per-run integration branches;
- preserve every intentional path removal recorded in
  `docs/upstream-intentional-removals.txt`;
- do not re-expose capabilities listed in
  `crates/konnect-core/src/router/registry.rs::DISABLED_TOOLS` without an
  explicit product decision;
- integrate behavior semantically in cohesive, independently verified slices;
- never stage or commit `.agents/` task, plan, or research files.

Keep detailed process steps in the runbook rather than duplicating them here.
