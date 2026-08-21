# Back up updated installed skills — 2026-08-21

- [x] Locate installed and repository skill directories
- [x] Compare every maintained skill file and identify installed improvements
- [x] Copy only changed source files into the repository backup
- [x] Validate skill structure and any changed detection scripts
- [x] Review the final diff and document verification results

## Review

- Backed up the installed two-layer review improvements in `SKILL.md` and
  `references/general-and-two-layer.md`.
- Added the installed PCB `references/layout-rules.md` and connected it to the
  skill entrypoint and embedded installer manifest so a fresh `konnect init`
  preserves it.
- Confirmed the package-audit and schematic skills already matched their
  installed copies; ignored runtime-only Python bytecode caches.
- All four skill directories pass `quick_validate.py` under UTF-8.
- The three copied files match their installed sources, `cargo fmt --check`
  passes, and both targeted manifest tests pass.
