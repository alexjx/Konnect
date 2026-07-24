# PCB design-document prerequisite

Use this contract before any Konnect PCB placement or layout mutation.

## Required document sections

The Markdown design authority must contain:

1. System purpose, operating conditions and frozen external requirements.
2. Functional block partition. For every block, list its central component,
   peripherals, owned nets, interfaces, scope and exclusions.
3. Complete selected BOM entries and package/footprint constraints for all
   placement-critical components.
4. Datasheet compliance table with manufacturer, document path or stable URL,
   relevant section/page, exact actionable layout requirement and design status.
5. Per-block PCB placement requirements and measurable acceptance checks.
6. Signal topology: connector-to-destination order, differential pairs, clock
   loops, analog paths, bus physical order, protection and termination points.
7. Power topology: sources, converters, current levels, star points, return
   paths, copper-width/plane requirements, thermal assumptions and test plan.
8. Mechanical baseline: board size or envelope, holes, keepouts, connector
   orientation, mating access, polarity, height and enclosure constraints.
9. Stack-up, impedance targets, net classes, clearances, via strategy and DFM
   constraints.
10. Placement and routing review checklist with objective pass/fail criteria.
11. Open decisions, each marked as blocking or non-blocking.

## Datasheet coverage

For every applicable device, check at least:

- supply bypass, bulk capacitance and ground-return requirements;
- clock, crystal, feedback, compensation and reset placement;
- connector-side ESD/TVS/filter placement and surge return;
- high-speed or differential routing topology and reference plane;
- analog separation, Kelvin sensing and low-current reference paths;
- switching-current loops, magnetics, thermal pad and heat spreading;
- high-current copper, connector pad neck-downs and via sharing;
- package land pattern, exposed pad, keepout and orientation;
- mechanical mating, polarity and assembly access;
- manufacturer-specific layout example and warnings.

Do not copy only schematic recommendations. The document must convert the
datasheet into PCB placement/routing instructions.

## Measurable acceptance

Datasheets often say "close" or "short" without a numerical limit. The project
document must retain that manufacturer language and add a clearly labeled
`PROJECT` target such as pad-edge distance, maximum loop dimension, via count,
required ordering or reserved corridor width. Do not present a project target
as a manufacturer limit.

## Stop conditions

Stop and tell the user what to add when any of these is true:

- no Markdown design authority exists;
- a functional block or placement-critical part is absent;
- a datasheet is named but has no actionable layout extraction;
- protection, termination, bypass or clock placement is ambiguous;
- mechanical holes, connector orientation or keepouts are undecided;
- current, stack-up or routing corridor requirements are missing;
- an unresolved choice would materially change placement;
- acceptance criteria are purely subjective, such as "looks compact".

Do not begin staging, moving, rotating, shrinking the outline or routing while
the gate is stopped.
