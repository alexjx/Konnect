---
name: kicad-layout-review
description: Read-only KiCad PCB layout audit with prioritized, evidence-backed findings. Use for general layout reviews, buck-converter layout, two-layer ground neck-down and return-path checks, or comparison with manufacturer datasheet layouts. Do not use for placement, routing, board edits, or fabrication execution.
---

# KiCad Layout Review

Review the exact PCB revision without changing it. Do not move footprints,
reroute copper, refill zones, save the board, or generate fabrication outputs.
If the user asks to fix findings, hand the findings to the applicable layout
skill in a separate mutation workflow.

## Establish evidence

1. Identify the exact `.kicad_pcb`, its saved/live state, layer count, stack-up,
   copper weight, fabrication constraints, and the scope of the review. State
   any fact that cannot be verified.
2. Inspect footprints, pads, nets, tracks, vias, keepouts, board geometry, and
   the final filled copper on all relevant layers. Zone outlines and DRC alone
   do not show the actual current or return path. If fills are stale, report
   that limitation instead of mutating the board.
3. Relate layout to the schematic and design requirements: identify power and
   return paths, switching nodes, sensitive signals, current levels, edge
   rates, thermal loads, controlled-impedance nets, and mechanical constraints.
   Do not infer unknown electrical limits from track geometry.
4. Read [general and two-layer checks](references/general-and-two-layer.md) for
   every review. Apply its two-layer section only when the board has two copper
   layers.
5. When a buck converter is present, also read
   [buck-converter checks](references/buck-converter.md).

## Datasheet authority

For each device whose placement, grounding, thermal pad, sensing, or high-speed
routing matters, use the exact manufacturer part number and package. Establish
the project-approved datasheet revision, then check the latest available
revision and errata for relevant changes; do not silently replace the project's
design authority. Inspect the layout section, package drawing, exposed-pad
requirements, reference schematic, and evaluation/reference board when
available. Record the revision, page, figure, table, or board layer supporting
each device-specific finding.

Treat a reference layout as evidence, not geometry to copy blindly. Compare its
layer count, stack-up, package, operating range, and populated options with the
reviewed design. The exact device documentation overrides generic practice;
explain any conflict. Never invent a numeric clearance, width, via count,
thermal limit, or current rating.

## Report only defensible findings

Run available read-only connectivity and DRC checks, writing reports only to a
disposable task location and preserving the board, live markers, fills, and
save state. Independently inspect placement, current loops, return continuity,
filled copper, thermal paths, EMI coupling, and manufacturer rules. A clean DRC
is not layout approval.

For each confirmed finding provide:

- severity: `BLOCKER` for a proven safety, functional, fabrication, or release
  criterion violation; `HIGH` for a likely functional, thermal, EMI, or
  reliability failure that should be fixed before release; `MEDIUM` for a
  documented material weakness without demonstrated failure; or `LOW` for a
  bounded improvement;
- confidence (`HIGH`, `MEDIUM`, or `LOW`) based on evidence completeness and
  how directly the governing source applies;
- exact location: reference, pad/net, layer, and coordinates or a marked view;
- observed geometry and the electrical, thermal, EMI, mechanical, or assembly
  risk it creates;
- governing evidence: project constraint, calculation, or datasheet page and
  figure; and
- the smallest practical remedy, without applying it.

Separate confirmed defects from `NEEDS EVIDENCE` items. Give each unconfirmed
item its potential severity, the exact missing evidence, and how to close it;
do not count it as a defect. End with review scope, limitations, DRC status,
remaining blockers, and whether the reviewed evidence supports release. Do not
claim release readiness while required evidence is missing or blocker/high
findings remain.
