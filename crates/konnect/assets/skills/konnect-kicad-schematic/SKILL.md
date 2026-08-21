---
name: konnect-kicad-schematic
description: Create, modify, or verify KiCad schematics through Konnect MCP. Use for schematic structure, symbols, wiring, component properties, placement, or electrical validation; use the guarded workflow whenever it supports the requested mutation.
---

# Konnect KiCad Schematic

Use Konnect for every schematic write. Never edit `.kicad_sch` text directly.
Do not automate the KiCad GUI unless the user explicitly requests it.

## Route by exposed capabilities

- When `inspect_design` is exposed, start with it and the exact project or
  schematic path. Do not call routing meta-tools in the guarded `workflow`
  profile; they are intentionally unavailable.
- Prefer `plan_schematic_edit` in `workflow` and `expert` for existing
  component Value, Footprint, BOM/DNP, or existing custom-field edits. It also
  supports relative symbol moves snapped to the 1.27 mm grid.
- The guarded workflow does not create fields or objects, edit wiring or
  hierarchy, rotate symbols, or move attached wires and labels with a symbol.
  In `workflow`, report unsupported work. In `expert`, use raw tools only for
  unsupported work or necessary read-only evidence. In `expert` or `legacy`,
  use `list_toolboxes` and `load_toolset` to expose the required raw tools.
  Never use raw tools to bypass a guarded rejection, stale plan, or failed
  verification.
- If one request mixes supported and unsupported mutations, do not apply only
  the supported subset unless the user explicitly accepts partial completion.

## Guarded change lifecycle

1. Inspect the exact resource, then create a reviewable typed plan.
2. Before applying, require `lifecycle: planned`, `effect_state: none`, the
   intended canonical resource and operations, an allow-list containing only
   authorized references, and the expected fingerprints/change summary.
   Planning must not write the schematic.
3. Apply with `apply_change_set`. Call `verify_change_set` only after the apply
   response reports `lifecycle: applied` and
   `effect_state: persisted_to_disk`. Completion requires `verified` and
   `persisted_to_disk`.
4. Use `get_change_set` whenever state is uncertain. `discard_change_set`
   abandons only an untouched plan; it is not rollback.
5. For stale or expired work, reinspect and create a fresh plan. For rejected or
   invalid work, correct the reported cause before replanning; never retry the
   same request unchanged. A verification failure occurs after the schematic
   was written, so inspect the file before any further write. On
   `effect_state: unknown` or error code `partial_apply`, stop mutations and
   recover from observed file state instead of retrying blindly. Change sets
   are process-local and normally expire after 30 minutes.

## Preserve design intent

- Treat the current schematic as authoritative. Keep edits surgical and
  preserve unrelated pages, objects, annotations, and user placement. Do not
  regenerate, reflow, or normalize existing work without explicit scope.
- For new layout, arrange each functional chain in a natural left-to-right row
  that follows signal or power flow. Keep support parts near the component they
  serve; short branches and uneven spacing are acceptable when they improve
  clarity.
- Place parts to keep local wires short and avoid crossings. Do not draw long
  wrap-around wire loops to escape a crossover. When unrelated connections
  would cross or obscure the flow, use matching net labels on short local
  stubs.
- Omit label rotation for normal placement so Konnect derives rotation and
  justification from the attached wire. Use an explicit rotation only for a
  deliberate exception, and preserve electrical label shape where relevant.
- Require authoritative design or datasheet evidence when a change affects
  topology, ratings, safety, interfaces, pin mapping, or package selection.
  Stop only when missing facts materially prevent a correct change. A clerical
  correction already established by maintained project authority does not
  require unrelated documentation work.
- Guarded `move_components` moves symbols only. When attached wires or labels
  must move with a symbol, use a connected-island operation only when raw tools
  are exposed; otherwise report the capability gap.

## Verify

After a write, inspect the exact targets and confirm unrelated content did not
change. Export changed pages when geometry or presentation changed, and run
connectivity/ERC checks when electrical connectivity could have changed. Scale
verification to the mutation, but do not declare completion from a successful
tool call alone. Before a raw write, identify the exact page and target objects;
if it returns an error or partial/ambiguous outcome, inspect observed state and
do not retry until the actual effect is known. If Konnect lacks the required
surgical or verification operation, stop instead of rebuilding the page or
using an unapproved fallback.
