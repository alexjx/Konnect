---
name: konnect-kicad-pcb-layout
description: Modify KiCad PCB layout and verify applied changes through Konnect MCP and live KiCad IPC. Use for footprint placement, board geometry, routing, zones, or fabrication preparation. Use konnect-kicad-layout-review instead for a read-only layout audit.
---

# Konnect KiCad PCB Layout

Mutate PCB data only through Konnect and the formal live KiCad PCB Editor.
Verify the exact requested board before writing; never edit `.kicad_pcb`
directly or use an unbound/background document.

## Route by exposed capabilities

- Start with the exact project or board path. Use `open_project`,
  `get_project_info`, and `get_board_info` to establish identity, then read the
  authorized footprints with `get_component_list`.
- Use `set_component_placements` for an atomic batch of existing-footprint
  transforms. Supply the complete absolute `x`, `y`, and `rotation` for every
  entry, even when only one value changes.
- Discover and load only the raw toolsets needed. The server's board-access
  classification decides whether an operation requires live IPC, permits a
  revision-checked closed-board fallback, or must refuse. Do not bypass that
  decision with direct file editing.
- Use specialized tools for flips, creation/deletion, outlines, routing, vias,
  zones, and netclasses. Do not infer support from a related placement tool.
- Do not apply only the supported subset of a mixed request unless the user
  explicitly accepts partial completion.

## Reviewable raw change lifecycle

1. Prove the exact board identity and read every authorized target. Take a
   project snapshot before a broad or difficult-to-reverse change.
2. Present the complete intended transforms and target references before
   writing. Raw tools do not create a stored plan, fingerprint, or courtyard
   baseline.
3. Apply one small batch. A live `set_component_placements` call is one KiCad
   undo step; a closed-board fallback is one revision-checked file write.
4. Re-read every affected footprint. If live IPC was used, save with
   `save_project` only after the readback and applicable checks pass; the raw
   placement call does not own the save.
5. Stop on a refusal, error, or ambiguous result and inspect observed state
   before another mutation. Use KiCad undo, the snapshot, or version control for
   recovery; there is no stored change set to discard.

## Scope and evidence

- Preserve unrelated placement and constrain every batch to authorized
  references. Do not turn a local repair into board-wide placement or outline
  work without explicit scope.
- Read the project's existing design authority and exact manufacturer,
  mechanical, and fabricator requirements that materially affect the requested
  change. Follow [the design-authority gate](references/design-document-contract.md)
  when required evidence is missing or conflicting.
- For placement or floorplanning, apply the maintained
  [layout rules](references/layout-rules.md) after project-specific requirements.
- Raw placement does not provide the fork workflow's new-overlap courtyard
  baseline. Check placement and courtyard evidence separately. Neither a clean
  origin-based move nor a screenshot proves assembly margin, board-edge
  clearance, keepouts, enclosure/access, copper clearance, thermal behavior, or
  electrical requirements.

## Raw work and completion

For raw writes, bind the exact live board, snapshot authorized targets, re-read
affected state afterward, and save through IPC only after applicable checks
pass. If a raw write returns an error or ambiguous outcome, stop, inspect the
live board, and do not retry until the actual effect is known. Read
[PCB completion](references/pcb-completion.md) only for routing, vias, zones,
DRC, or fabrication outputs. Do not order or transmit fabrication without
explicit user authorization.
