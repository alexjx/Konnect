# Bundle all Konnect skills in `konnect init` — 2026-08-27

- [x] Inspect the installer path and current embedded skill manifest
- [x] Add the prefixed package-audit skill and every required resource
- [x] Update README installation documentation
- [x] Add an isolated installer test covering all bundled files
- [x] Validate skills, compile tests, formatting, and final diff

## Review

- Added `konnect-kicad-package-audit` to the embedded manifest with its
  frontmatter, UI metadata, configuration reference, and both maintained scripts.
- Updated the README to document all four skills installed by `konnect init`.
- Added an isolated installer test that writes every embedded file and verifies
  its exact content for all four skills and removes the two legacy unprefixed
  directories.
- All four skills pass `quick_validate.py`; `cargo fmt --check` and the complete
  `cargo test -p konnect` suite pass.
- Ran `cargo run -p konnect -- init` against the active installation. It reported
  four installed skills, and the subsequent `status` command showed `[+]` for
  every prefixed skill. Confirmed no legacy skill directories remain.
