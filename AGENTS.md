# Repository Agent Instructions

These instructions apply to the whole repository. Keep this file focused on
stable operating rules; use the linked documents for implementation details and
current inventories.

## Start here

Konnect is a Rust MCP server and packaging project for KiCad. Before making a
non-trivial change:

1. Read [`docs/DEVELOPER_OVERVIEW.md`](docs/DEVELOPER_OVERVIEW.md) for the system
   map, then the relevant focused guide below.
2. Inspect `git status` and preserve unrelated or uncommitted user work.
3. Record the plan and verification evidence under `.agents/tasks/` (research
   belongs under `.agents/research/` and longer plans under `.agents/plans/`).
4. Keep `.agents/` local and untracked. Never stage or commit it.

The main references are:

- [`DEV.md`](DEV.md) for setup and the detailed contributor map;
- [`CONTRIBUTING.md`](CONTRIBUTING.md) for public contracts and the CI gate;
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for ownership and data flow;
- [`docs/TOOL_SYSTEM.md`](docs/TOOL_SYSTEM.md) and
  [`docs/DEVELOPING_TOOLS.md`](docs/DEVELOPING_TOOLS.md) for MCP tools;
- [`docs/KICAD_INTEGRATION.md`](docs/KICAD_INTEGRATION.md) for file, IPC, and
  `kicad-cli` boundaries;
- [`docs/TESTING_AND_RELEASE.md`](docs/TESTING_AND_RELEASE.md) for validation;
- [`docs/VERSIONING.md`](docs/VERSIONING.md) for fork version derivation and
  release synchronization;
- [`docs/NAMING_CONVENTIONS.md`](docs/NAMING_CONVENTIONS.md) for public names.

## Code ownership and data flow

Put behavior in the layer that owns it:

- `crates/konnect`: process entrypoint, CLI, configuration, transports,
  installation, status, and plugin FFI;
- `crates/konnect-core`: MCP handling, routing and exposure policy, tool
  definitions, workflows, observability, and backend selection;
- `crates/konnect-sexp`: common KiCad file parsing, atomic edits, reversible
  commands, and transaction journals;
- `crates/konnect-schematic-editor`: typed schematic model and mutations;
- `crates/konnect-ipc`: typed protobuf messages, NNG transport, exact-document
  targeting, and transport failure classification;
- `crates/schematic-viewer`: separate Tauri application, excluded from the
  Cargo workspace;
- `plugin` and `packaging`: KiCad launcher/settings integration and PCM
  assembly/validation.

The usual runtime flow is transport -> `McpHandler` -> `ToolRouter` -> domain
handler -> file, IPC, or `kicad-cli` backend. Keep policy in `konnect-core` and
reusable transport/file primitives in their lower-level crates.

## Non-negotiable data-safety rules

- Existing schematic writes must be revision-aware and atomic. Use a transaction
  journal for multi-file changes; a changed source becomes a conflict, never an
  overwrite.
- A live board mutation must prove that the requested board is the active board
  before it writes.
- Hybrid board operations may fall back to a closed-file edit only when IPC is
  unreachable. If KiCad received, rejected, or timed out on a request, fail
  closed because it may already own or have changed the document.
- File-only board tools must refuse to edit a board currently open in KiCad.
- Preserve complete KiCad/protobuf objects across read-modify-write operations.
  Risky IPC mutations require a bounded read-back when the immediate response
  does not prove the committed state.
- Success, counts, and clean verdicts must come from parsed, committed, or
  read-back evidence—not echoed request values. Missing prerequisite evidence
  means incomplete or diagnostic, not success.
- Use real KiCad-produced fixtures for file formats and reports. Synthetic
  fixtures are appropriate only for narrow grammar cases.

Validate every target and argument before the first mutation, and group a live
mutation into one undo transaction when the existing API supports it. Follow
existing guarded helpers instead of rebuilding these checks ad hoc. The safety
policy and reference implementations are documented in
[`docs/KICAD_INTEGRATION.md`](docs/KICAD_INTEGRATION.md).

## Public contracts and tool changes

Treat MCP tool names and schemas, CLI flags, environment variables, config keys,
and documented paths as public API. A rename needs compatibility or an explicit
migration decision.

For MCP handlers:

- read required values with the typed `require_*` helpers or `get_path`; do not
  silently default a required argument;
- use `try_layer_from_name` for write-path layer conversion;
- preserve stable structured error classifications across layers;
- update definitions, registry/exposure policy, `tool-directory.md`, tests, and
  every affected public surface such as skills, hooks, manifests, and user
  guidance together.

Do not copy tool totals into new guidance. The registry and tool directory are
authoritative. When the tool surface changes, run `cargo xtask fix-doc-counts`
instead of editing derived counts by hand, then run:

```text
cargo test -p konnect --test doc_tool_counts
```

Do not re-expose capabilities listed in
`crates/konnect-core/src/router/registry.rs::DISABLED_TOOLS` without an explicit
product decision. Disabled names must remain absent from discovery, dispatch,
hooks, skills, and the public tool directory. Workflow tools are a separate
guarded surface; do not add them to raw toolsets merely to make them
discoverable. Keep discovery and direct dispatch consistent for every exposure
profile.

## Implementation and verification

Make the smallest cohesive change that fixes the root cause. Reuse neighboring
patterns and add the lowest-level test that proves the risky behavior, followed
by handler/protocol coverage when a public contract changes. Test refusal and
conflict paths as deliberately as success paths.

Run focused checks first. Before declaring a repository-wide change complete,
run the applicable CI baseline:

```text
cargo check --workspace --locked
cargo test --workspace --locked --lib --tests
cargo test --workspace --locked --doc
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

The toolchain is pinned in `rust-toolchain.toml`; `konnect-ipc` also requires
`protoc` and its well-known includes. The viewer is outside the workspace, so
viewer changes additionally require:

```text
cargo check --locked --manifest-path crates/schematic-viewer/Cargo.toml
cargo test --locked --manifest-path crates/schematic-viewer/Cargo.toml
```

Use `uv`, never the system Python, for plugin and packaging checks:

```text
uv run --python 3.11 python -m unittest discover -s plugin/tests -v
uv run --with jsonschema python packaging/validate-pcm.py --metadata packaging/metadata.json
```

Changes to packaged files or release scripts also require building and
validating a representative PCM archive. Live KiCad tests are environment-
dependent: run them only against a disposable document, and explicitly report
them as not run when unavailable. A normal workspace test does not prove live
GUI or IPC behavior.

Before handoff, inspect the final diff, run `git diff --check`, list the exact
tests run and skipped, and explain the evidence behind behavioral claims.

## Git and upstream integration

- Use explicit paths when staging; never use `git add -A` in this repository.
- Do not discard, rewrite, or include unrelated user changes. Avoid destructive
  Git commands unless the user explicitly requests them.
- In a shared worktree, only the coordinating agent may switch branches, stage,
  or commit. Give parallel agents disjoint file ownership and serialize Cargo
  commands unless they use separate `CARGO_TARGET_DIR` values.
- `main` is the release line. `dev` is the single permanent upstream-integration
  line; do not create a new integration or topic branch for each upstream run.
- Before fetching, merging, cherry-picking, replaying, or reviewing upstream
  changes, read and follow
  [`docs/UPSTREAM_INTEGRATION.md`](docs/UPSTREAM_INTEGRATION.md).
- Preserve every intentional removal recorded in
  [`docs/upstream-intentional-removals.txt`](docs/upstream-intentional-removals.txt).
  Do not restore a removed path merely because upstream still contains it.
- Integrate upstream behavior semantically in cohesive, independently verified
  slices. Preserve product and safety contracts, not obsolete implementation
  text, and never resolve broad conflicts with repository-wide `ours` or
  `theirs`.

The integration runbook is the authority for repeatable steps, checkpoints,
rollback, and the full validation ladder. Improve that runbook when an
experiment produces a demonstrably safer or clearer workflow.
