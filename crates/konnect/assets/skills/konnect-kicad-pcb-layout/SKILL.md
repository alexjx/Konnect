---
name: konnect-kicad-pcb-layout
description: Use Konnect KiCad MCP and live KiCad IPC to review, stage, place, compact, route, verify, and prepare PCB designs for fabrication. Apply to .kicad_pcb placement, footprint or outline changes, routing, vias, netclasses, copper zones, DRC, manufacturing preflight, or checks against datasheet, mechanical, signal-integrity, power, thermal, and DFM requirements.
---

# Konnect KiCad PCB Layout

Use Konnect MCP and KiCad IPC as the only PCB mutation path. Treat placement as
an engineering constraint problem: prove the design requirements first, place
by functional block, then compact without violating those requirements.

## Konnect discovery and task routing

- Prefer Konnect's workflow interface when available: inspect with `inspect_design`, create a typed plan with `plan_pcb_edit`, review the returned change set, then use `apply_change_set` and `verify_change_set`. Never bypass a stale-plan, exact-document, allow-list, or verification failure.
- In `expert` or `legacy` exposure mode, start with `list_toolboxes` and `get_active_toolsets`; load only current toolsets needed for the task. Use raw tools only for a capability the workflow interface does not yet support, while preserving the same preflight and verification gates.
- Never guess a workflow, tool, or toolset name. Rediscover the available interface after an unavailable-capability error.
- For routing, zones, final DRC, or fabrication output, also read and follow [`references/pcb-completion.md`](references/pcb-completion.md).

## Mandatory design-document gate

Locate and read the project's complete Markdown design authority before opening
a placement workflow. Apply the contract in
[`references/design-document-contract.md`](references/design-document-contract.md).

Stop before modifying the PCB if the document is missing or incomplete. Tell
the user exactly which sections or device requirements must be completed. Do
not infer missing datasheet requirements, create a provisional placement, or
use the existing PCB as a substitute for the design document.

The gate must establish all of the following:

- functional blocks, scope, central component, peripherals and interfaces;
- manufacturer datasheet sources and actionable PCB layout requirements;
- measurable placement and review acceptance criteria;
- signal flow, power paths, physical bus order and termination points;
- board outline, holes, keepouts, connector orientation and mating access;
- stack-up, current paths, routing classes, thermal needs and DFM constraints;
- blocking open decisions, explicitly identified as unresolved.

## Konnect and IPC preflight

1. Use the workflow inspection interface, or in expert/legacy mode load only the PCB toolsets needed.
2. Require the formal project PCB Editor to be visibly open. A project manager
   window or an invisible/background document is insufficient.
3. Verify that the active IPC board document is the exact requested
   `.kicad_pcb`. If Konnect cannot prove the document identity, stop and ask the
   user to open the formal PCB Editor; do not continue against an assumed board.
4. Read board outline, footprint list, positions, rotations, pads, layers and
   available bounds through Konnect. Before any placement mutation, require
   live IPC access to each affected footprint's transformed courtyard geometry
   and body bounds. Read-only file inspection is allowed for diagnosis, but
   never write the PCB file.
5. Use IPC commits for every move, rotation, creation, deletion, outline change
   and save. Never use direct `.kicad_pcb` edits, temporary PCB windows, hidden
   fallbacks or parse-and-write workarounds.
6. If an IPC operation is unsupported or returns ambiguous status, stop that
   operation and report it. Fix Konnect or request direction before continuing.
7. Treat an origin-to-origin distance, pad bounding box or nominal package size
   as insufficient evidence of physical clearance. If Konnect cannot return or
   test live transformed courtyards, stop placement; do not estimate clearance
   from centers or screenshots.

## Preserve existing work

- Apply a local-only mutation policy to every board with formal placement
  already underway. Never rebuild, globally reflow, or re-stage the complete
  board merely because this skill was invoked again.
- Treat formal placement as started when any block has been deliberately
  grouped, mechanical anchors have been accepted, the user has manually moved
  parts, or a prior placement pass has been saved. An unrouted board is not
  automatically an unplaced board.
- Preserve user-positioned footprints that are not part of the reported issue.
- Before a batch, snapshot all footprint positions and declare an allow-list of
  the exact references that may move, rotate, be created or be deleted.
- Before committing a proposed position, query the moved footprint courtyard
  and every nearby stationary courtyard. Reject the proposal if courtyards
  intersect or the required courtyard-to-courtyard gap is not met.
- Limit the allow-list to the reported part, its directly dependent support
  parts, and the smallest set of immediate neighbors required to restore a
  documented clearance or topology constraint.
- Keep batches small enough to verify visually, normally one functional block
  or a smaller local subcircuit at a time.
- Record the moved, rotated, created and deleted references. Treat partial IPC
  success as a failure requiring inspection before another batch.
- After every batch, diff the complete position snapshot. If any reference
  outside the allow-list changed, stop, report the unexpected mutation and do
  not continue compacting.
- Do not move the board outline, mounting pattern, fixed connectors, unrelated
  blocks or completed manual placement during a local repair unless the user
  explicitly includes them in scope.
- If a local correction cannot satisfy the design constraints, stop and explain
  the global conflict. Propose the smallest scope expansion; do not silently
  convert the task into a whole-board redesign.

## Placement workflow

### 1. Build a constraint map

Translate the design document into a per-block placement checklist. Separate:

- `DATASHEET`: explicit manufacturer requirements;
- `PROJECT`: measurable local targets adopted by the design;
- `MECHANICAL`: fixed outline, holes, connectors and keepouts;
- `ROUTING`: required corridors, reference planes, current widths and topology.

Never replace phrases such as "as close as possible" with arbitrary spacing.
Use the project acceptance target when supplied; otherwise stop and request a
measurable target or document the proposed target for user approval.

### 2. Establish mechanical anchors

Place and lock the board outline, mounting holes, screw-head keepouts,
edge-mounted connectors, polarized power connectors, switches and mandatory
access volumes first. Verify complete footprint bodies and courtyards, not only
origins. Connector origins are often asymmetric.

Confirm connector opening direction, polarity, cable bend space, screwdriver
access, insertion/removal direction and board-edge overhang from the official
drawing or 3D model.

### 3. Create external staging areas

Use external staging only under either of these conditions:

1. formal placement has not started anywhere on the board; or
2. the user explicitly requires full or named-block re-staging/re-layout.

Do not interpret a poor local placement, unrouted ratsnest, available external
space, or a request to "review/fix/compact" as permission to stage the board
again. When formal placement already exists, repair the affected footprint or
smallest affected block in place.

When staging is authorized, stage footprints outside the outline before final
placement:

- create one compact, labeled spatial cluster per functional block;
- place the block's central component at the cluster core;
- arrange its peripherals around that core by electrical relationship;
- order repeated channels and paired components consistently;
- keep staged groups separated so ownership is unambiguous;
- do not scatter unplaced footprints around the board.

Staging outside the board is temporary and deliberate. No staged footprint may
remain outside the final outline. Do not move unrelated placed blocks into a
staging area. If the user authorizes re-staging only one named block, preserve
every other block exactly.

### 4. Place each functional block

Use the central component as the organizing core unless a connector or power
entry is the stronger physical anchor.

- IC-centered block: place the IC, then its bypass, clock, reset, feedback and
  local support parts directly around the corresponding pins.
- Connector/protection block: place from the board edge inward in physical
  signal order: connector, protection/filtering, series elements, destination.
- Power block: place input connector, conversion or star node, energy-storage
  parts and loads in current-flow order; reserve copper and thermal space first.
- Repeated channel block: use identical orientation, pitch, ordering and local
  topology unless a documented constraint requires an exception.

Keep each block visually and electrically coherent. A part belongs near the
component or connector it serves, not in unrelated empty space.

### 5. Integrate blocks

Orient blocks so their interfaces face each other and ratlines cross as little
as practical. Preserve explicit signal flow and physical bus order. Reserve:

- continuous reference planes for high-speed and sensitive signals;
- differential-pair and clock corridors without plane splits or stubs;
- wide high-current paths and via-stitching fields;
- separation between switching/high-current loops and sensitive analog, clock,
  USB or CAN circuitry;
- routing channels for dense packages before closing surrounding whitespace.

Read and apply all common rules in
[`references/layout-rules.md`](references/layout-rules.md).

### 6. Compact without wasting space

Compact only after every block passes its local datasheet checklist.

- Reduce whitespace inside each block first, then between complete blocks.
- On an existing placed board, compact only the requested local block unless
  the user explicitly authorizes a broader compaction pass.
- Use actual courtyard/body clearances plus the documented assembly margin.
- Keep repeated gaps uniform and align repeated rows or columns.
- Move a coherent block as a unit when closing global whitespace; do not pull
  bypass, protection, clock or feedback parts away from their served pins.
- Justify every large empty area as a routing corridor, copper-current region,
  thermal area, keepout, isolation distance or mechanical access volume.
- Treat unjustified whitespace as a placement defect.
- Treat excessive compression that damages routing, return paths, thermal
  spreading, accessibility or datasheet proximity as an equal defect.
- Shrink or redraw the board outline only after the final footprint and corridor
  bounding boxes pass. Never choose a small outline first and force the circuit
  into it.
- Never shrink the whole outline as an incidental consequence of a local fix.
  Outline changes require explicit scope and a board-wide keepout/body review.

## Geometry rules

- Align movable footprints to the project grid unless a connector drawing or
  controlled-impedance geometry requires another coordinate.
- Search existing official, shared, and project libraries before creating a
  footprint. Prefer project scope for a new footprint and verify pad numbers,
  dimensions, courtyard, mask/paste, pin-1 marking, polarity, and 3D orientation
  against the exact manufacturer package drawing.
- Check bodies, pads, courtyards, fabrication outlines and 3D envelopes. Never
  infer fit from a footprint origin or center cross.
- Reject body/courtyard overlap, board-edge overhang, screw-keepout intrusion,
  inaccessible connectors and silkscreen obscuring pads.
- Courtyard non-overlap is a hard placement invariant, not a final-review item.
  Check it before and after every individual move or rotation. Do not continue
  the batch after the first collision.
- Use transformed `F.CrtYd`/`B.CrtYd` geometry from the active IPC document.
  Axis-aligned bounding boxes may be used only as a broad-phase test: disjoint
  boxes prove separation, but overlapping boxes require exact geometry testing
  or a stop. Never clear a placement from footprint centers alone.
- Keep paired or same-function parts adjacent, aligned, consistently oriented
  and unmistakably related.
- Keep local ratlines short, direct and topologically ordered; do not optimize
  only the total ratline length.
- Rotate parts to support routing and assembly. Verify pad and graphic movement
  after every rotation because a changed origin/angle alone is not proof.

## Verification loop

After each meaningful batch:

1. Re-read all changed footprint positions, rotations and pad geometry through
   Konnect.
2. Run live courtyard collision checks for every changed footprint against all
   board footprints and keepouts. Require an explicit zero-collision result.
3. Render or inspect the visible PCB Editor. The rendered body is authoritative
   when API coordinates and visible geometry disagree.
4. Check the affected block against its `DATASHEET`, `PROJECT`, `MECHANICAL` and
   `ROUTING` constraints.
5. Check complete bodies against the outline and all keepouts.
6. Confirm no unrelated footprint moved.
7. Save through KiCad IPC only after the batch passes.

Before declaring placement complete, run the checklist in
[`references/layout-rules.md`](references/layout-rules.md), DRC as applicable,
and a final visual review at useful zoom. Report unresolved constraints rather
than hiding them with a compact-looking placement.
