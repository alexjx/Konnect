---
name: konnect-kicad-schematic
description: Create, modify, or verify KiCad schematics through Konnect MCP. Use for schematic structure, symbols, wiring, component properties, placement, or electrical validation.
---

# Konnect KiCad Schematic

Use Konnect for every schematic write. Never edit `.kicad_sch` text directly.
Do not automate the KiCad GUI unless the user explicitly requests it.

## Route by exposed capabilities

- Start with `get_project_info` and the exact project or schematic path. Load
  only the required raw toolsets with `list_toolboxes` and `load_toolset`, then
  read the exact targets using `get_schematic_component`,
  `list_schematic_components`, or `get_schematic_layout`.
- Use `batch_edit_schematic_components` for existing Value, Footprint, or
  custom-field edits. The upstream integration baseline does not expose a
  guarded BOM/DNP field mutation; report that gap instead of forcing it through
  a generic field edit.
- Use `bulk_move_schematic_components` for one relative batch offset or
  `move_schematic_component` for one absolute target. These operations move
  symbols, not their attached wires and labels.
- Use the specialized schematic, wiring, hierarchy, and library tools for other
  supported work. Follow their server-side validation and refusal results; do
  not bypass a rejected operation with text editing or GUI automation.
- If one request mixes supported and unsupported mutations, do not apply only
  the supported subset unless the user explicitly accepts partial completion.

## Reviewable raw change lifecycle

1. Inspect the exact file and target objects. Take a project snapshot before a
   broad or difficult-to-reverse change.
2. State the intended operation and authorized references before writing. Raw
   tools do not create a stored plan or allow-list.
3. Invoke the smallest suitable atomic or batch operation once, and retain its
   exact response as evidence.
4. Re-read the affected objects immediately. If the response is partial,
   ambiguous, or reports an error, stop and recover from observed file state
   instead of retrying blindly.
5. Run the relevant connectivity, rendered-layout, and ERC checks before
   declaring completion. Use the snapshot or version control for rollback;
   there is no stored change set to discard.

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
- Component move tools move symbols only. When attached wires or labels must
  move with a symbol and no verified connected-island operation is available,
  report the capability gap.

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
