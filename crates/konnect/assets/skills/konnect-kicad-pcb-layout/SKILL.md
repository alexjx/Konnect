---
name: konnect-kicad-pcb-layout
description: Modify KiCad PCB layout and verify applied changes through Konnect MCP and live KiCad IPC. Use for footprint placement, board geometry, routing, zones, or fabrication preparation. Use konnect-kicad-layout-review instead for a read-only layout audit.
---

# Konnect KiCad PCB Layout

Mutate PCB data only through Konnect and the formal live KiCad PCB Editor.
Verify the exact requested board before writing; never edit `.kicad_pcb`
directly or use an unbound/background document.

## Prefer the guarded workflow

For complete absolute transforms of existing footprints, use this lifecycle in
Expert or Workflow:

1. Open the exact board in the visible PCB Editor, then call `inspect_design`
   with its exact path. Confirm the live document identity and authorized
   footprint references.
2. Call `plan_pcb_edit` with the same board and complete absolute placement for
   every target. Planning is zero-write and must uniquely resolve every target.
3. Review the returned `change_set_id`, immutable operations, exact resource
   revision, courtyard/overlap evidence, and every structured gate. Use
   `get_change_set` when retained state must be re-read. Do not apply a blocked,
   incomplete, or stale plan.
4. Call `apply_change_set` once with that ID. It must bind the exact live board
   and commit the complete placement update as one KiCad undo step.
5. Call `verify_change_set` with the same ID. Completion requires exact live
   document readback and a verified effect state, not merely a successful apply.

If a zero-effect plan is no longer wanted, call `discard_change_set`. Change
sets are process-local and expire; never treat an ID from another server
process as durable authorization.

## Raw tools for unsupported operations

The guarded PCB schema currently supports existing-footprint transforms.
Flips, pad angles, creation/deletion, outlines, routing, vias, zones, and
netclasses require exposed raw tools in Legacy or Expert. Workflow has no raw
router; report the capability gap instead of changing profiles or calling a
hidden implementation without user direction.

For a supported raw operation, establish the exact board identity, load only
the required toolset, inspect every target, and invoke the smallest atomic or
batch write once. The server's board-access classification decides whether an
operation requires live IPC, permits a revision-checked closed-board fallback,
or refuses. Re-read every affected object and stop on any refusal, partial
result, or ambiguous effect. Do not apply only the supported subset of a mixed
request unless the user explicitly accepts partial completion.

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
- Raw placement does not provide the guarded workflow's new-overlap courtyard
  baseline. Check placement and courtyard evidence separately. Neither a clean
  origin-based move nor a screenshot proves assembly margin, board-edge
  clearance, keepouts, enclosure/access, copper clearance, thermal behavior, or
  electrical requirements.

## Raw work and completion

For raw writes, bind the exact live board, snapshot authorized targets, re-read
affected state afterward, and save through IPC only after applicable checks
pass. If a raw write returns an error or ambiguous outcome, stop and inspect
the live board. Read
[PCB completion](references/pcb-completion.md) only for routing, vias, zones,
DRC, or fabrication outputs. Do not order or transmit fabrication without
explicit user authorization.
