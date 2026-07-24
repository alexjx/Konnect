# PCB routing, review, and fabrication completion

Read this reference when the task includes routing, vias, netclasses, copper
zones, DRC, manufacturing review, or fabrication exports. Keep the project
design document, datasheets, stack-up, and selected fabricator's current rules
authoritative over generic defaults.

## Pre-routing gate

- Complete and verify mechanical anchors and functional-block placement first.
- Confirm the schematic-to-PCB connectivity is current and every required
  footprint is assigned.
- Translate documented electrical requirements into named netclasses,
  clearances, widths, via constraints, differential-pair rules, impedance
  targets, and length or skew limits. Do not invent generic dimensions.
- Verify the intended layer stack, reference planes, controlled-impedance
  assumptions, high-current requirements, and fabrication capabilities.
- Run a baseline DRC and distinguish pre-existing violations from new work.
- Reserve return paths, high-current copper, thermal spreading, sensitive
  analog regions, and critical routing corridors before ordinary routing.

## Routing order

Route in risk order:

1. fixed mechanical and connector escape constraints;
2. power entry, switching loops, high-current paths, and critical returns;
3. clocks, differential pairs, controlled-impedance and other sensitive nets;
4. analog feedback, sense and compensation networks;
5. buses and remaining ordinary signals.

Keep critical loops short and topologically recognizable. Preserve a continuous
reference plane under high-speed signals, minimize layer transitions, and place
return vias beside unavoidable signal-layer transitions. Route differential
pairs from documented impedance, gap, width, skew and length constraints; do
not rely on a universal USB, Ethernet, CAN, or LVDS recipe.

Use named netclasses for repeatable constraints instead of setting unrelated
per-track values. Verify pad numbers and net identity before every route.
Inspect each committed route in the live editor and re-read its geometry.

## Vias and copper zones

- Select via type, drill, annulus and layer span from current design and
  fabricator constraints. Avoid unnecessary vias in high-current and
  high-speed paths.
- Create a zone only after its net, layer, clearance, thermal policy, keepouts,
  current role, and plane-return consequences are understood.
- Treat ground zones as return-path structures, not automatic empty-space
  fill. Do not allow plane splits, narrow necks, isolated islands, accidental
  antenna stubs, or copper inside mechanical keepouts.
- Check thermal relief behavior for current capacity and solderability.
- Refill zones after relevant placement, routing, outline, keepout, or rule
  changes. Inspect the rendered fill and re-run DRC.

## Structured design review

Review the board in escalating order:

1. structural integrity: outline closure, footprints, references, unrouted
   items, courtyard/body collisions, keepouts and board-edge clearance;
2. electrical safety: shorts, clearances, current paths, return paths, plane
   continuity, polarity, protection and exposed-pad connections;
3. signal integrity: impedance, differential pairing, skew, clocks, stubs,
   crosstalk exposure and layer transitions;
4. power and thermal behavior: switching loops, bulk and local decoupling,
   copper area, thermal vias, heat spreading and temperature-sensitive parts;
5. manufacturing and assembly: pad/mask/paste geometry, pin-1 and polarity
   marks, silkscreen, test access, fiducials, tooling and connector access.

Classify every finding as `CRITICAL`, `WARNING`, or `SUGGESTION`. Name the
affected reference, pad, net, layer, coordinates or rule, cite the governing
datasheet/project/fabricator requirement, and propose the smallest safe repair.
Do not declare the board ready while critical findings remain.

## DRC and fabrication package

1. Save the verified live board through IPC.
2. Refill zones and run DRC against the intended final rules and stack-up.
3. Resolve every error. Document intentional warnings and waivers with their
   engineering basis; never hide or silently suppress them.
4. Validate footprint assignment, polarity, pin 1, courtyard, edge clearance,
   solder mask, paste, silkscreen, drill treatment, assembly side, and
   component availability.
5. Generate fabrication and assembly outputs from one saved revision:
   Gerbers or the requested fabrication format, drill files, BOM, component
   position files, drawings, and 3D output as applicable.
6. Inspect the generated layer set and board outline with an independent
   viewer. Confirm plated/non-plated drills, mask and paste layers, coordinate
   origin, units, rotations, side designators, and BOM-to-position consistency.
7. Compare the package against the selected fabricator and assembler's current
   documented requirements. Do not hard-code one vendor's historical limits.
8. Report the exact output location, revision, remaining waivers, and whether
   the package is ready for submission. Do not place an order without explicit
   user authorization.
