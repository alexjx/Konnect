# Upstream Integration Workflow

This runbook defines how this fork repeatedly adopts changes from
[`mixelpixx/Konnect`](https://github.com/mixelpixx/Konnect) without restoring
features or guidance that the fork intentionally removed.

The integration model is **semantic replay**: start from the current upstream
tip, then rebuild only the fork behavior that is still required. A normal merge
or a bulk replay of every fork commit is not the default because both sides have
changed the same architectural areas independently.

## Workflow status

- **Stable constraints** are product and safety invariants. An experiment must
  not change them implicitly.
- **Experimental steps** may be reordered, split, combined, or automated when a
  run produces evidence that the change is safer or easier to review.
- Record each run's checklist and outcome in `.agents/tasks/todo.md`, with raw
  investigation notes under `.agents/research/`. Promote only evidence-backed,
  reusable process rules into this document.

Changing a stable constraint requires an explicit product decision and a
separate documentation change.

## Stable constraints

1. Never integrate directly on `main`; use the permanent `dev` branch.
2. The first integration establishes `dev` at the fetched
   upstream tip. Later integrations continue on that same branch instead of
   creating per-run topic branches. Do not rebuild or reset the permanent branch
   without an explicit recovery decision.
3. The paths in
   [`upstream-intentional-removals.txt`](upstream-intentional-removals.txt) must
   be absent from the final tree. Useful upstream guidance may be adapted into
   the fork's namespaced replacements, but the removed paths are not restored.
4. Removal policy applies to the callable capability surface as well as files.
   Every name in `router::registry::DISABLED_TOOLS` must remain absent from MCP
   discovery, dispatch, Claude hook matchers, skills, and the public tool
   directory unless an explicit product decision re-enables it.
5. Preserve behavior, tests, and data contracts—not old implementation text.
   Prefer a newer upstream implementation when it satisfies the same contract.
6. Integrate one cohesive behavior slice at a time. Each slice must be reviewable
   and independently tested before the next begins.
7. Do not resolve broad conflicts using repository-wide `ours` or `theirs`.
8. Do not carry fork-only version bumps or `.agents/` bookkeeping into the new
   product history.
9. Keep the old fork tip recoverable until the integrated release is verified.
10. Do not declare the run complete until the full validation ladder and the
   intentional-removal assertion pass.

## What may be optimized experimentally

The following are workflow choices rather than product policy:

- the size and ordering of behavior slices;
- which isolated leaf commits are cherry-picked versus reapplied manually;
- how equivalence between fork and upstream behavior is measured;
- how often the slower validation levels run;
- automation for inventory, conflict classification, and invariant checks;
- the structure of diagnostic worktrees or per-slice scratch layouts.

Record the reason, observed result, and decision whenever one of these changes.

## Phase 0: preflight

Run from the repository root in PowerShell. Stop if the worktree is not clean;
commit or deliberately preserve unrelated work before continuing.

```powershell
git status --short --branch
git remote -v
git branch -vv
```

Configure the parent repository once if an `upstream` remote is not already
present:

```powershell
git remote add upstream https://github.com/mixelpixx/Konnect.git
git fetch upstream --prune --tags
```

On later runs, only the fetch is required. Confirm that `origin` is still the
fork and `upstream` is still the parent before using either ref.

Choose a run identifier and create a temporary notes file under
`.agents/research/`. Record the exact fork tip, upstream tip, toolchain, and
operating system.

```powershell
$runId = Get-Date -Format "yyyyMMdd-HHmmss"
```

When parallel agents share one worktree, assign mutually exclusive file ranges.
Only the coordinating agent may change branches, stage, commit, or clean files.
Serialize Cargo commands that use the default target directory, or give each
agent a distinct `CARGO_TARGET_DIR`. Before every commit, inspect both the
worktree and the exact staged file list:

```powershell
git status --short
git diff --cached --name-only
```

Use explicit paths with `git add`; never use `git add -A` during an integration
run. Prefer `git grep` or explicit tracked product paths for inventory and
documentation scans so `.agents/`, `target/`, and generated output cannot alter
the result.

## Phase 1: measure the divergence

Capture the following evidence before choosing any implementation strategy:

```powershell
$forkTip = git rev-parse main
$upstreamTip = git rev-parse upstream/main
$base = git merge-base $forkTip $upstreamTip

git show -s --format="fork %H %ad %s" --date=iso-strict $forkTip
git show -s --format="upstream %H %ad %s" --date=iso-strict $upstreamTip
git show -s --format="base %H %ad %s" --date=iso-strict $base
git rev-list --left-right --count "$forkTip...$upstreamTip"
git diff --stat "$base..$forkTip"
git diff --stat "$base..$upstreamTip"
git diff --name-status "$base..$forkTip"
git diff --name-status "$base..$upstreamTip"
git cherry -v $upstreamTip $forkTip
```

Classify the overlap by subsystem: manifests and dependencies, MCP/router,
schematic operations, PCB/IPC operations, S-expression editing, installer and
skills, plugin/packaging, and documentation.

### Optional merge probe

For a heavily diverged run, measure real merge behavior in a disposable
worktree. This is diagnostic only; never continue implementation in the probe.

```powershell
$probe = Join-Path (Resolve-Path .agents/tasks) "upstream-merge-probe"
git worktree add --detach $probe $forkTip
git -C $probe merge --no-commit --no-ff $upstreamTip
git -C $probe status --short
git -C $probe merge --abort
git worktree remove $probe
```

If the merge succeeds unexpectedly, still compare behavior and policy before
adopting it. A clean textual merge does not prove semantic compatibility.

## Phase 2: create or resume the permanent integration line

Create a recoverable marker for the current product tip. On the first run only,
create `dev` from upstream and restore the fork-only workflow
files. On every later run, switch to the existing branch and continue from its
verified tip.

```powershell
git tag "fork-before-upstream-$runId" $forkTip
$integrationBranch = "dev"
$existing = git branch --list $integrationBranch
if ($existing) {
    git switch $integrationBranch
} else {
    git switch -c $integrationBranch $upstreamTip
    git restore --source=$forkTip -- docs/UPSTREAM_INTEGRATION.md docs/upstream-intentional-removals.txt
}
```

Publish the permanent branch to the fork with
`git push -u origin dev` after its first validated commit. Do
not push the safety tag automatically; decide whether it should be published
when the integration is ready for team review. Restoring the two workflow files
is required only on the first run because a branch rooted at upstream does not
contain fork-only documentation yet.

Some fork clones fetch only `origin/main`. In that case `push -u` writes branch
configuration but Git still refuses to resolve `@{u}` because the integration
branch is outside the remote's fetch refspec. Add the permanent branch to the
refspec once, then verify that ahead/behind checks use the real remote-tracking
reference:

```powershell
$integrationRefspec = "+refs/heads/dev:refs/remotes/origin/dev"
if ((git config --get-all remote.origin.fetch) -notcontains $integrationRefspec) {
    git config --add remote.origin.fetch $integrationRefspec
}
git fetch origin dev
git rev-parse --abbrev-ref --symbolic-full-name '@{u}'
git rev-list --left-right --count HEAD...origin/dev
```

For later runs, record which upstream commit the branch last integrated. Measure
only the new upstream interval and choose merge, cherry-pick, or semantic replay
per behavior slice. Frequent, small updates may merge cleanly; the permanent
branch does not make a blind merge acceptable.

## Phase 3: establish fork policy first

The first product slice makes the intended final policy visible:

1. Remove every tracked path in `docs/upstream-intentional-removals.txt`:

   ```powershell
   $legacyPaths = Get-Content docs/upstream-intentional-removals.txt
   $trackedLegacyPaths = @($legacyPaths | ForEach-Object { git ls-files -- $_ })
   if ($trackedLegacyPaths.Count -gt 0) { git rm -- $trackedLegacyPaths }
   ```

2. Add or update the five namespaced replacements:
   - `konnect-kicad-schematic`
   - `konnect-kicad-pcb-layout`
   - `konnect-kicad-layout-review`
   - `konnect-kicad-package-audit`
   - `konnect-kicad-symbol`
3. Adapt the namespaced manifests and auxiliary files to upstream's current
   installer model. Do not transplant the old installer wholesale.
4. Import relevant upstream safety or tool-signature corrections into the
   namespaced skills without recreating the removed directories.
5. Compare the fork's callable tool surface with the upstream implementation
   catalog. Restore explicit disabled-name tombstones before registering new
   workflow or profile surfaces; an implementation may remain for controlled
   migration without becoming discoverable or dispatchable.

After the slice, this command must produce no output:

```powershell
$restoredLegacyPaths = @(
    Get-Content docs/upstream-intentional-removals.txt |
        ForEach-Object { git ls-files -- $_ }
)
$restoredLegacyPaths
if ($restoredLegacyPaths.Count -gt 0) {
    throw "Intentional upstream removals were restored"
}
```

Treat any output as a failed invariant, not as a documentation discrepancy.

## Phase 4: classify fork commits by behavior

Assign every fork-only commit or behavior to exactly one disposition:

| Disposition | Meaning | Required evidence |
|---|---|---|
| Accept upstream | Upstream satisfies the same or stronger contract | Matching tests or a focused behavior comparison |
| Port behavior | The fork capability remains valuable but its implementation conflicts with upstream architecture | New implementation and regression tests on the upstream base |
| Cherry-pick leaf | The change is isolated and does not import stale architecture | `git show --check`, a small diff, and targeted tests |
| Drop | Version bump, bookkeeping, obsolete implementation, or intentionally retired behavior | Reason recorded in the run notes |

Patch identity alone is insufficient. Independently implemented fixes will not
have matching patch IDs even when upstream has superseded the fork behavior.
Treat the old fork's inputs, outputs, errors, and regression tests as contract
evidence, not its helpers, types, or control flow as design authority. Map the
contract onto upstream's current document/session, transaction, and typed-IPC
primitives before writing an adapter.

Good leaf candidates are isolated documentation additions and the distinct
`Konnect Settings` plugin action. Cross-cutting IPC, router, installer, manifest,
and workflow commits require semantic review.

## Phase 5: replay cohesive slices

Use this default order because each later slice consumes contracts established
by the earlier ones:

1. fork policy, intentional removals, and namespaced skills;
2. unique schematic audits or correctness behavior;
3. document/session binding and typed IPC primitives;
4. live PCB geometry operations not already supplied upstream;
5. the guarded inspect-plan-apply-verify workflow layer;
6. raw capability filtering, workflow exposure profiles, and hook isolation;
7. plugin naming, packaging validation, tailored README, and contributor docs.

For each slice:

1. State the behavior and its inputs, outputs, mutations, and failure modes.
2. Identify the upstream implementation points and tests it must compose with.
3. Add or adapt a focused regression test that proves the behavior contract.
4. Implement the smallest coherent change.
5. Run the appropriate validation level below; do not commit a failing slice.
6. Review the diff against both upstream and the old fork implementation.
7. Commit the slice independently, with source fork commits noted in the message
   or run log. Do not combine unrelated policy, plugin, schematic, or IPC work.

Any tool addition, removal, or rename must update the registry, tool directory,
README, developer guide, package metadata, and plugin manifest wherever they
state the public surface. Run `cargo test -p konnect --test doc_tool_counts`
before committing that slice; record the current values in product files, not
as permanent numbers in this workflow.

If a port starts reproducing an upstream subsystem, stop and redesign it around
the upstream abstraction instead.

## Phase 6: validation ladder

### Level A — every slice

- Run the narrowest relevant unit or integration test.
- Run `cargo fmt --all -- --check` for Rust changes.
- Run `git diff --check`.
- Re-run the intentional-removal assertion for installer or asset changes.

### Level B — subsystem milestone

```powershell
cargo check --workspace --locked
cargo test --workspace --locked --lib --tests
cargo test --workspace --locked --doc
cargo clippy --workspace --locked --all-targets -- -D warnings
```

For viewer changes:

```powershell
cargo check --locked --manifest-path crates/schematic-viewer/Cargo.toml
cargo test --locked --manifest-path crates/schematic-viewer/Cargo.toml
```

For plugin or package changes, use the project-managed Python environment:

```powershell
uv run --python 3.11 python -m unittest discover -s plugin/tests -v
uv run --with jsonschema python packaging/validate-pcm.py --metadata packaging/metadata.json
```

### Level C — release candidate

```powershell
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked --lib --tests
cargo test --workspace --locked --doc
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo build --release --locked -p konnect
git diff --check
$restoredLegacyPaths = @(
    Get-Content docs/upstream-intentional-removals.txt |
        ForEach-Object { git ls-files -- $_ }
)
if ($restoredLegacyPaths.Count -gt 0) {
    $restoredLegacyPaths
    throw "Intentional upstream removals were restored"
}
```

The final removal command must print nothing. Then run the applicable ignored
KiCad 10 end-to-end, conformance, fixture, IPC, and fork-specific workflow tests.
Finish with one disposable-board live IPC smoke cycle covering inspect, plan,
apply, verify, and recovery behavior.

## Phase 7: final review and adoption

Before replacing `main`, review:

```powershell
git log --oneline --decorate "$upstreamTip..HEAD"
git diff --stat "$upstreamTip..HEAD"
git diff --name-status "$upstreamTip..HEAD"
git diff --stat "$forkTip..HEAD"
$restoredLegacyPaths = @(
    Get-Content docs/upstream-intentional-removals.txt |
        ForEach-Object { git ls-files -- $_ }
)
$restoredLegacyPaths
if ($restoredLegacyPaths.Count -gt 0) {
    throw "Intentional upstream removals were restored"
}
```

The review must answer:

- Which old fork behaviors are now supplied by upstream?
- Which behaviors were ported, and which tests prove them?
- Which behaviors were deliberately dropped?
- Are all intentional removals still absent?
- Are the namespaced skills accurate for the integrated tool schemas?
- Are README, packaging metadata, and plugin behavior consistent?
- Does the full validation record identify every skipped environment-dependent
  test and why it was skipped?

Adopt the integration branch only after these answers and the validation evidence
have been reviewed. Keep the pre-integration marker until the new release has
passed normal use.

## Recovery

Because work is sliced into commits on a separate branch, prefer reverting a bad
slice or rebuilding the integration branch from the recorded upstream tip. Do
not repair a failed experiment by rewriting `main` or discarding the old fork
marker.

## Workflow provenance

### 2026-09-01 discovery baseline

- Divergence: 36 fork-only commits and 666 upstream-only commits
- Trial merge: 46 content conflicts and 15 delete/modify conflicts
- Intentional removals: 17 paths; all exist upstream, 15 were modified upstream
- Capability removals: 37 raw tool names in the old fork; 33 implementations
  still exist upstream and must be explicitly filtered rather than silently
  re-exposed
- Decision: use semantic replay from upstream; do not direct-merge or replay all
  36 commits mechanically
- Confidence: 0.94 against direct merge; 0.88 for semantic replay; 0.80 for
  individual keep/drop choices before behavior-level testing
