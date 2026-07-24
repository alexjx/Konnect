---
name: konnect-kicad-schematic
description: Use Konnect KiCad MCP to create, modify, review, and validate hierarchical KiCad schematics, including project setup, library selection, pages, symbols, placement, wiring, labels, structured electrical review, and ERC repairs.
---

# Konnect KiCad Schematic

Use Konnect for every schematic write. Never edit `.kicad_sch` text directly. Do not open or automate the KiCad GUI unless the user explicitly requests it; validate with Konnect, `kicad-cli`, and exported pages.

## Konnect discovery and preflight

- Start with `list_toolboxes` and `get_active_toolsets`; load only the current toolsets required by the task.
- Use `project` and `config` for project state and design rules; use `sch_components`, `sch_wiring`, `sch_analysis`, `sch_batch`, `sch_export`, `sch_hierarchy`, `library`, `templates`, `verification`, and `design_review` as applicable.
- Do not guess a tool or toolset name. If a capability is missing, rediscover the current registry and stop if Konnect cannot perform the required write safely.
- Identify the exact `.kicad_pro` and target page before mutation. Re-query the changed objects afterward and save through Konnect.

## Existing work and design authority

- Treat current schematic files as authoritative. Builder scripts are historical recipes.
- Treat an existing-page repair as surgical: inspect first, name the exact target items, and preserve everything else.
- Never regenerate, clear, reflow, or broadly normalize an existing page without explicit permission.
- Use `move_schematic_connection_island` for a connected group; do not move only its symbol and stretch the circuit.
- Query current bounds immediately before and after any placement or field edit.

Before schematic writes, require one Markdown design document containing the functional partition, page responsibilities, interfaces, selected major parts and datasheets, required peripheral circuits, documented deviations, and unresolved design decisions. Stop if missing information affects topology, ratings, safety, or interfaces.

## Hierarchy

- Root page: use pinless child-sheet blocks only; no circuit components or internal nets.
- Give every root-page sheet block the same width and height. Choose the common width from the longest displayed title plus clear horizontal padding; no title may be wider than its block, and shorter titles do not produce shorter blocks.
- Put each sheet title outside and above its block. Align the title's left edge with the block's left edge and keep a consistent vertical gap between the title baseline and the block's top edge.
- Verify equality and containment from rendered title and block bounds, not from nominal coordinates alone.
- Arrange sheet boxes in page/signal-flow order with even spacing. Reserve the lower-right title block and description area; no sheet box, title, wire, or note may overlap it.
- Child page: one independently understandable and testable function with one clear central subject.
- Split a page when it contains multiple central circuits, repeated measurement channels beside a controller, or label congestion that prevents short readable connections.
- Keep related circuits compact and grid-aligned. Do not solve congestion by shrinking text or scattering components.

## Standard schematic grammar

These are mandatory defaults unless the circuit topology makes them impossible:

- Power enters from the top; ground/return leaves at the bottom.
- Inputs and sources are on the left; outputs and loads are on the right.
- Signal and energy flow is left to right. Grow the main path as one predominantly straight horizontal trunk; do not step it vertically merely to reach a conveniently placed part.
- Put series-path resistors, inductors, diodes, ferrites, shunts, and switches horizontally along that flow. In particular, keep a converter's switch node, series inductor, output rail, shunt, and load on the same horizontal reading direction whenever topology permits.
- Put shunt parts vertically as short branches off the trunk. This includes rail-to-return capacitors and diodes whose role is freewheel, catch, clamp, TVS, or other node-to-return action; it does not include a diode that is actually in the series path.
- A switching converter must visibly read left to right: input/control -> switching device -> horizontal inductor or other series power element -> output/sense/load. The trunk contains the major parts and only short, visually simple branches.
- Keep the converter output rail horizontal. Place output capacitors vertically below or above that rail; do not turn the output path into a vertical maze.
- Leave a horizontal outward stub from every horizontal pin before the first bend. Never turn at the pin endpoint or route through a symbol body.
- Prefer rotating or moving a component over adding doglegs. Use orthogonal wires only.
- Avoid U-shaped, rectangular, or other wrap-around wires that enclose symbols or loop around the main circuit. First reposition or rotate the related components so the important connection is short and direct.

### Direct functional networks versus label branches

Classify by topology, not capacitance or perceived importance:

- Always directly wire the main series path. Directly wire other identifiable functional networks only when the result is compact and does not wrap around the main circuit: switch nodes, gate drive, compensation, feedback dividers, current sense, and explicit RC/LC/π filters.
- Treat all non-decoupling parts that implement one local function as one functional island. First rotate, mirror, and place the active devices into a recognizable topology, then directly wire their internal nodes and arrange the associated series, bias, pull, and feedback parts around the same trunk. This includes MOS/BJT gate drivers, Darlington-like stages, level shifters, and transistor switch networks; do not split their internal gate, base, or collector nodes with repeated labels merely because the parts use different device types.
- Draw every voltage divider and matched differential input network as one visible functional unit. Keep its related branches adjacent, parallel, ordered consistently (`+` above `-` when horizontal), and equally spaced; align matching resistors and connect them directly to their source and destination pins when compact. Do not scatter or separate paired sense resistors with labels merely because the two branches are different nets.
- A filter capacitor is direct only when its relationship with the series R/L and the filtered node is part of the topology a reader must see.
- A capacitor or capacitor bank connected only between an already named supply rail and return is decoupling/bypass, including local converter input/output rail banks. Draw it as a separate vertical branch or grouped bank using rail and return labels; do not merge it into the main series-path island.
- Bootstrap, charge-pump, compensation, timing, and snubber capacitors are not decoupling. Keep them visibly grouped with their related pins, but use paired labels with short stubs when direct wiring would create a large loop, surround another symbol, or cross unrelated main-path nets.
- Never replace a readable short direct connection with labels. Use labels inside one functional chain only after repositioning cannot avoid a large wrap-around loop, a crossover, or intrusion into unrelated circuitry. Conversely, never preserve direct wiring by drawing a large return loop; simplify placement first, then use paired labels for the remaining auxiliary branch.
- Use labels for cross-page nets, long/complex connections, and peripheral decoupling branches. Do not use paired labels to hide a short, simple series path.
- Treat ground as a distributed named net, not as a repeatedly drawn common node. Terminate each ordinary ground/return branch with its own nearby `GND` label; do not extend a ground wire, draw a ground bus, or converge unrelated branches merely to reuse one ground connection. For repeated vertical branches, place the labels at the branch bottoms and align their anchors horizontally whenever practical.

## Symbols

- Prefer, in order: verified KiCad official symbol, existing shared/project symbol, new project symbol.
- Search the active libraries before creating a symbol. Use project-scoped libraries by default so an individual design does not silently modify global libraries.
- Check the current `templates` toolset before rebuilding a standard circuit, but verify every template value, pin, footprint, and assumption against the selected device and project requirements.
- Verify exact pin numbers, names, package, exposed pad, and footprint against the local datasheet.
- Never guess pin numbers from memory or a similar device. Inspect the selected symbol and reconcile it with the exact device datasheet before wiring.
- Never use a connector or numbered rectangle as an IC substitute when a verified symbol exists.
- For a custom rectangular IC, put only true supply pins on top: explicit VCC/VDD/AVCC/AVDD/VIN/VBAT or datasheet-defined supply inputs. Put only GND/AGND/PGND/DGND and exposed ground pads on the bottom.
- Put all functional inputs and controls on the left, including EN/CE, reset, mode/configuration, feedback/sense inputs, bootstrap pins, charge-pump control/capacitor pins, and timing/compensation inputs. Bootstrap is not a supply category.
- Put outputs, status, switch nodes, and drivers on the right. Do not classify a pin as top/bottom merely because its name contains `V`, it participates in power conversion, or it connects to a capacitor.
- Group related pins and size the body so all pin text is clear.
- Keep pins with the same or closely related function contiguous at the small pitch, normally 2.54 mm center-to-center.
- Separate clearly different functional groups with one additional empty pitch, normally 5.08 mm center-to-center. Grow the symbol body as needed instead of compressing the groups or their text.
- If the datasheet does not establish a reliable functional distinction, use the small pitch uniformly; do not invent groups or arbitrary gaps.
- Set the reference prefix, value, footprint, datasheet, description, manufacturer part number, and procurement identifier when the project requires them. Use correct electrical pin types because ERC depends on them.
- For a custom symbol, make pin 1 and functional pin groups unambiguous, check every unit and hidden power pin, and render the symbol alone before use. For a custom footprint, verify pad numbers, package dimensions, pin-1 marking, courtyard, paste/mask behavior, and 3D orientation against the manufacturer drawing.
- Render a new custom symbol by itself before placing it.

## Placement and clearance

- Treat each symbol plus fields, pin text, first wire segments, and attached labels as one connection envelope.
- Keep unrelated symbols, wires, and text outside that envelope with at least one active-grid interval of visible clearance.
- Use `get_schematic_connection_islands(clearance=2.54)` before and after placement changes. Review every reported conflict.
- AABB conflicts from L-shaped islands may be false positives only after exported visual inspection proves that rendered graphics do not intersect.
- Keep repeated channels adjacent, aligned, symmetric, and in predictable name/pin order.
- Never let a wire cross a symbol body or approach a passive through its body. It may meet only the intended external pin lead.

## Fields

- Keep Reference and Value close to their symbol without touching body, pins, wires, labels, or each other.
- For resistors, capacitors, inductors, diodes, and other small two-pin passives, judge spacing from the rendered text bounds to the transformed symbol body, never from field-anchor distance alone.
- Target a visible body-to-text gap of 0.5-1.27 mm for small two-pin passives. Do not exceed 1.27 mm unless a wire, pin corridor, or nearby item forces it; never move a field farther merely to match a large-symbol convention.
- Keep both fields horizontally readable regardless of passive orientation. For a horizontally growing two-pin passive, put Reference above and Value below, centered on the body when possible. For a vertically growing passive, place both fields together on the clearer side, aligned close to the body.
- Treat the field pair as part of the passive's visual footprint: keep Reference and Value mutually aligned, separated, and compact; do not leave the default oversized 2.54-5.08 mm visual gap around a small body.
- For a central IC or large connector, put Reference near the upper-left and Value near the nearest clear lower edge, avoiding pin corridors.
- After placing or rotating a passive, run `check_schematic_field_spacing` with `min_clearance=0.2 mm` and `max_clearance=1.27 mm`, then inspect `get_schematic_symbol_bounds` for exceptions. Verify the exported page as well; spacing-tool success alone is insufficient.

## Power and net labels

- Never attach a label directly to a pin. Use the shortest clear stub, normally 2.54 mm and no more than 5.08 mm without a routing reason.
- Use `GND` as the distributed system-ground net for all consumer ground pins, converter returns, ADC references, output negative terminals, and ordinary return branches. Express these connections with repeated named `GND` global labels on short local stubs, not by scattering `power:GND` symbols as simple nodes and not by wiring branches to a shared drawn ground node. Do not distribute a battery-protection terminal name such as `P-`, `PACK-`, `B-`, or `BAT-` as a substitute for `GND`.
- Keep protection-domain names local and semantic: the raw cell negative (`B-`/`BAT-`) remains on the unprotected side of the shunt or protection switch; the protected pack-negative endpoint (`P-`/`PACK-`) appears only at the protection boundary. On the protection page, connect that protected endpoint directly to `GND` exactly once; use `GND` everywhere downstream and across pages.
- Do not add a resistor, jumper, or net-tie merely to join the protected negative endpoint to `GND`. A direct named-node connection is correct unless the design document explicitly requires galvanic separation, selectable isolation, or a PCB net-tie for layout control.
- Before validation, enumerate ground-related labels on every changed page. Reject any downstream `P-`/`PACK-` or raw `B-`/`BAT-` used as circuit ground. If KiCad reports the intentional protected-endpoint/`GND` alias at the single connection, record that warning and confirm the generated netlist chooses `GND`.
- Directional global/hierarchical labels must point toward the approaching wire:

  | Wire approaches label from | Tip faces | Body extends | Rotation | Justification |
  | --- | --- | --- | ---: | --- |
  | Left | left | right | 0 | left |
  | Right | right | left | 180 | right |
  | Top | up | down | 270 | right |
  | Bottom | down | up | 90 | left |

- Apply the table to all directional global/hierarchical labels, including distributed ground and positive-supply labels; it does not apply to plain local net labels or `PWR_FLAG`.
- Use `input` as the mandatory shape for every global label, independent of signal direction, page, connected pin type, or whether the net carries power. This project convention prioritizes one stable visual form and deliberately does not encode ERC direction in global-label shapes.
- Do not use `output`, `bidirectional`, `tri_state`, or `passive` global-label shapes unless the user explicitly overrides this convention for a specific project.
- Before final validation, enumerate every global label and reject any shape other than `input`. Normalize legacy and mixed shapes to `input`; do not infer direction from their existing shapes.
- Use named global labels for both positive supply rails and ground. Do not use ordinary `power:VCC`, `power:+3V3`, `power:+5V`, `power:GND`, or similar power symbols as connection nodes; express the rail name with repeated global labels on short local stubs.
- Every power-rail global label uses the same mandatory `input` shape. Its orientation follows the approaching-wire table, not the conventional graphic of the power symbol it replaces.
- For ground below a component: pin -> short vertical wire downward -> `GND` global-label tip facing upward -> label body below. Use shape `input`, rotation `270`, and right justification, then verify the exported outline.
- A direct wire between nearby ground pins is allowed only when it represents one compact, readable local return branch. Terminate that branch once with a `GND` label. Do not place a separate `GND` label on every pin of the same compact branch, and do not join unrelated branches into a large ground island.
- A vertical shunt diode or capacitor should normally terminate in its own short return stub and return label. Reusing the same net name is electrically identical; prefer this local branch over a long horizontal ground wire. Align sibling return labels on one horizontal row unless doing so would lengthen or cross the main trunk.

### `PWR_FLAG`

- `PWR_FLAG` is the only ordinary `power:*` symbol retained by this convention. It is an ERC source marker, not a substitute for a named supply or ground label.
- Treat `PWR_FLAG` only as ERC metadata.
- Put it in the rail-source/power page, grouped in a quiet corner, never on a consumer merely because it uses the rail.
- Use only `rail label -> short local wire -> PWR_FLAG`. Never attach it to a main IC, functional rail trunk, decoupling capacitor, connector, or load.
- Keep different rail declarations separate and visually isolated.

## Electrical review

- Derive required support circuitry from the exact datasheet and design document. Treat common values and patterns as prompts to investigate, not universal rules.
- Review every active device for supply pins, decoupling and bulk capacitance, biasing, reset/configuration pins, unused inputs, exposed pads, clocks, pull-ups on open-drain nets, and required protection or termination.
- Trace critical signals and power paths end to end. Check orphaned items, single-pin nets, unintended shorts, duplicate references, missing footprints, and intentionally unused pins without explicit no-connect markers.
- Classify findings as `CRITICAL`, `WARNING`, or `SUGGESTION`. Name the affected page, reference and pin or net; cite the governing datasheet or project rule; and propose the smallest safe Konnect operation.
- Do not declare the schematic complete while critical findings remain. Record justified waivers instead of suppressing evidence.

## Workflow and validation

1. Audit the design document, project configuration, libraries, applicable templates, and current page.
2. Identify the central subject and left-to-right topology before placing anything.
3. Verify symbol identities and pin maps. Place symbols using top-power/bottom-ground/left-input/right-output grammar; reserve connection envelopes.
4. Use batch operations only for three or more already-verified repetitive targets. Snapshot or enumerate the exact targets first; inspect partial success before another batch.
5. Draw the straight horizontal main series path first, including horizontal inductors and other series elements. Add short vertical shunt branches next; give simple ground-connected branches local, horizontally aligned return labels instead of a shared long return. Reposition other auxiliary parts to avoid wrap-around wiring; use short direct wires where compact and paired labels only for the remaining long auxiliary loops. Add vertical decoupling branches and cross-page labels afterward.
6. Annotate only after the placement structure is stable. Check duplicate references and preserve deliberate existing annotations.
7. Query connection islands and fix real envelope conflicts.
8. Export every changed page. Check flow direction, symbol/field overlap, label tips, pin stubs, and every passive connection visually.
9. Run orphan/dangling, single-pin-net, duplicate-reference, and shorted-net analysis. Resolve every unexplained result.
10. Run the electrical review above, then ERC. Record every remaining warning and waiver in the design document.
11. Save a verified checkpoint after each coherent batch and a final checkpoint only after the exported-page and electrical checks pass.

If Konnect lacks a required surgical operation, stop and report the missing capability instead of rebuilding the page or using the GUI.
