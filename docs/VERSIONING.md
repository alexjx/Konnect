# Versioning

Konnect uses one numeric product version across the Rust workspace, schematic
viewer, and PCM package. The fork version must identify its upstream baseline,
sort after that baseline, and satisfy KiCad's PCM schema, which accepts only
`major.minor.patch` with a numeric patch of at most six digits. SemVer
pre-release (`-xinj.N`) and build metadata (`+xinj.N`) therefore cannot be the
canonical product version.

## Fork version formula

For upstream version `U_MAJOR.U_MINOR.U_PATCH`, derive the fork version as:

```text
U_MAJOR.U_MINOR.(50000 + U_PATCH * 100 + FORK_REVISION)
```

`FORK_REVISION` starts at `1` for each integrated upstream patch and may range
from `1` through `99`. `U_PATCH` must be between `0` and `154`; crossing that
hard limit requires a new version line rather than a wider patch value.

Examples:

| Upstream baseline | Fork revision | Product version |
|---|---:|---|
| `0.11.0` | 1 | `0.11.50001` |
| `0.11.0` | 2 | `0.11.50002` |
| `0.11.1` | 1 | `0.11.50101` |
| `0.12.0` | 1 | `0.12.50001` |

The `50000` fork band makes a fork build visibly distinct while preserving
normal SemVer ordering. It also keeps every version component within the
16-bit limit used by Tauri's Windows `FILEVERSION`; the PCM schema's larger
six-digit limit is not the only constraint. An upstream minor release still
sorts after every fork release from the previous minor, and each integrated
upstream patch has 99 ordered fork revisions. The formula is for compatible
releases on the same version line; a fork change that requires a breaking
version bump must advance the appropriate major or minor component instead of
hiding that break in the encoded patch.

Do not continue the old `0.1.3-xinj.N` sequence. It no longer identifies the
integrated upstream baseline and KiCad PCM cannot accept it.

## Source of truth and synchronized files

The canonical in-development version and its exact provenance live in the root
`Cargo.toml`:

- `[workspace.package].version` is the product version;
- `[workspace.metadata.konnect-version]` records `upstream_version`, the full
  `upstream_commit`, and `fork_revision` used by the formula.

Keep these files synchronized when preparing a product build:

- `Cargo.toml` and the generated root `Cargo.lock`;
- `crates/schematic-viewer/Cargo.toml`;
- `crates/schematic-viewer/tauri.conf.json` and its generated `Cargo.lock`;
- the version passed to `packaging/build-pcm.ps1` or `build-pcm.sh`.

`packaging/metadata.json` is the published release catalogue, not the
in-development version source. Update its platform entries, URLs, sizes, and
hashes together only when publishing those artifacts. The PCM build scripts
stamp the selected product version into the package-local metadata.

## Release procedure

1. Record the exact integrated upstream version and full commit in
   `[workspace.metadata.konnect-version]`.
2. Derive the next version with the formula above. Reset `FORK_REVISION` only
   when `U_PATCH` changes; otherwise increment it.
3. Update the synchronized source files and regenerate both Cargo lockfiles.
   The `version_contract` integration test validates the formula, provenance,
   viewer config, and local lockfile packages.
4. Run the applicable validation in `docs/TESTING_AND_RELEASE.md`.
5. Build the release binary and PCM archive with the same version.
6. Verify `konnect --version`, the package metadata, and the installed binary
   all report that version before tagging or publishing.

Never reuse a product version for different source. If a build must be replaced,
increment `FORK_REVISION` even when the upstream baseline is unchanged.
