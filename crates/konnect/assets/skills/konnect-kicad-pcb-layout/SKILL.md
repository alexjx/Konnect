---
name: konnect-kicad-pcb-layout
description: Modify KiCad PCB layout and verify applied changes through Konnect MCP and live KiCad IPC. Use for footprint placement, board geometry, routing, zones, or fabrication preparation; use the guarded workflow whenever it supports the requested mutation. Use kicad-layout-review instead for a read-only layout audit.
---

# Konnect KiCad PCB Layout

Mutate PCB data only through Konnect and the formal live KiCad PCB Editor.
Verify the exact requested board before writing; never edit `.kicad_pcb`
directly or use an unbound/background document.

## Route by exposed capabilities

- When `inspect_design` is exposed, start with the exact project or board path.
  Require the requested board to be bound in the formal PCB Editor; if the
  returned path, `live_open`, or editor state cannot prove identity, stop.
- Prefer `plan_pcb_edit` in `workflow` and `expert` for existing-footprint
  absolute `x`/`y` and/or rotation transforms. It does not support relative
  moves, flips, layer changes, creation/deletion, outlines, routing, vias,
  zones, or netclasses.
- In `workflow`, report unsupported work. In `expert`, use raw tools only for
  unsupported operations or required read-only evidence. In `legacy`, discover
  and load only the raw toolsets needed. Never use raw tools to bypass a guarded
  rejection, stale fingerprint, courtyard gate, or failed verification.
- Do not apply only the supported subset of a mixed request unless the user
  explicitly accepts partial completion.

## Guarded change lifecycle

1. Inspect the exact live board, then create one small `plan_pcb_edit` batch.
2. Before applying, require `lifecycle: planned`, `effect_state: none`, the
   intended canonical board and absolute transforms, an allow-list containing
   only authorized references, the live fingerprint, and the courtyard
   baseline. Planning must not mutate KiCad.
3. Apply with `apply_change_set`. Call `verify_change_set` only after the apply
   response reports `lifecycle: applied` and `effect_state: live_document`.
   Apply creates one live undo transaction; successful verification owns the
   save. Do not save separately. Completion requires `verified` and
   `persisted_to_disk`.
4. Use `get_change_set` whenever state is uncertain. `discard_change_set`
   abandons only an untouched plan; it is not rollback.
5. For stale or expired work, reinspect and create a fresh plan. For rejected or
   invalid work, correct the reported cause before replanning; never retry the
   same request unchanged or bypass it with raw tools. On
   `effect_state: unknown` or error code `partial_apply`, stop mutations and
   recover from observed live state. On `save_failed` with
   `effect_state: live_document`, preserve the open board and retry verification
   only after correcting the save problem. For any other verification failure,
   inspect and preserve the live board before another write.

## Scope and evidence

- Preserve unrelated placement and constrain every batch to authorized
  references. Do not turn a local repair into board-wide placement or outline
  work without explicit scope.
- Read the project's existing design authority and exact manufacturer,
  mechanical, and fabricator requirements that materially affect the requested
  change. Follow [the design-authority gate](references/design-document-contract.md)
  when required evidence is missing or conflicting.
- Guarded courtyard validation proves only that the transforms introduce no new
  courtyard overlaps. It does not prove assembly margin, board-edge clearance,
  keepouts, enclosure/access, copper clearance, thermal behavior, or electrical
  requirements. Obtain separate evidence when those constraints matter; do not
  infer clearance from origins, nominal package sizes, or screenshots.

## Raw work and completion

For raw writes, bind the exact live board, snapshot authorized targets, re-read
affected state afterward, and save through IPC only after applicable checks
pass. If a raw write returns an error or ambiguous outcome, stop, inspect the
live board, and do not retry until the actual effect is known. Read
[PCB completion](references/pcb-completion.md) only for routing, vias, zones,
DRC, or fabrication outputs. Do not order or transmit fabrication without
explicit user authorization.
