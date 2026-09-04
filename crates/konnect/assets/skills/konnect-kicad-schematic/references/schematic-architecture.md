# Schematic Architecture Workflow

Read this file before creating a new schematic, substantially redrawing one,
or converting between single-sheet and hierarchical structure.

## 1. Reuse survey

Inspect the current workspace before inventing a page structure or visual
grammar. Search for, in order:

1. The target project's design authority, AGENTS.md, existing schematic, and
   rendered schematic outputs.
2. Closely related maintained projects and their root sheets, child sheets,
   block-design documents, and schematic-layout standards.
3. Shared application notes and exact reusable circuits for selected devices.

Record a compact reuse map containing the target function, source project and
page, what is reused unchanged, what must be adapted, and why. Reuse proven
page composition and circuit grouping when the topology is the same. Do not
copy stale values, pin maps, footprints, or project-specific constraints just
because the drawing looks suitable.

## 2. Choose single-sheet or hierarchy

A single sheet is appropriate when the complete circuit has one cohesive flow,
few independent interfaces or power domains, and remains readable at a normal
review scale. Plan its visual zones before placing symbols.

Use a hierarchy when the design has several independently reviewable
functions, reusable circuits, multiple power or interface domains, or would
become crowded on one reviewable page. Do not split merely to reach a preferred
page count. Do not retain a flat sheet merely because ERC passes.

State the chosen mode and the concrete readability or coupling reason.

## 3. Complete architecture proposal

Prepare the whole schematic architecture before component-level drawing. It
must include:

- system energy flow, signal/data flow, power domains, ground domains, and
  safety or isolation boundaries;
- root-page purpose and visual arrangement;
- every page name and filename;
- each page's single responsibility, central device or object, included
  circuits, and explicit exclusions;
- ownership of every external connector;
- cross-page net/interface table with source, consumers, direction, voltage
  domain, default state, and any analog/noise constraint;
- ownership of rail generation, power flags, reset/enable defaults, protection,
  and measurement references;
- a reuse map tied to existing workspace pages or shared application circuits;
- the intended composition of every page, not merely a list of sheet names;
- per-page and integrated verification steps.

The architecture must account for all selected functional circuits. A block
list that leaves circuits unassigned is incomplete.

## 4. Root-page patterns

Follow the closest maintained workspace convention unless the target design
requires a different one.

- Pinless-block root: the root contains only named functional sheet blocks and
  short architecture notes. Child pages connect through controlled global net
  labels. This is useful when the workspace already documents cross-page nets
  separately and the root is intended as a compact visual index.
- Ported-block root: sheet pins and short root wires show important energy and
  signal flow. Use this when explicit sheet interfaces improve review and do
  not turn the root into a wiring page.
- Single sheet: no decorative root or empty index page. The circuit page is the
  project entry page.

For a hierarchy, the root is an architecture view. Do not place ordinary
component circuits on it. Global power and ground conventions must be explicit
in the architecture document even when they are not drawn across the root.

## 5. Child-page composition

Define the layout of each page before drawing it, then preserve the same visual
grammar across the project:

- inputs and upstream sources on the left;
- main IC, controller, converter, or protected object near the center;
- outputs, downstream loads, and output connectors on the right;
- positive supplies above and ground/returns below when this clarifies flow;
- decoupling, bootstrap, feedback, pull-up/down, filtering, and protection next
  to the pins or connector they serve;
- complete direct wiring inside each functional cluster as the default: a
  reviewer must be able to follow the local circuit without joining a field of
  repeated labels mentally;
- matching short-stub labels for cross-page nets and intentionally separated
  clusters, and for any local connection that would otherwise cross unrelated
  content, span large empty space or require a wrap-around route;
- consistent power/global labels for positive rails and ground rather than long
  rail wires; `PWR_FLAG` is reserved for a necessary ERC source assertion on an
  already labelled rail and is not used as the visible rail marker;
- analog/measurement clusters visibly separated from switching nodes;
- high-current switch, shunt, Kelvin pickup, and current amplifier kept together
  when their physical/electrical relationship is part of correctness.

For each page, describe its left, center, right, upper, and lower zones when
those zones are applicable. Preserve a proven source page's relative grouping
when reusing the same circuit; adapt only what the new design changes.

## 6. Approval and implementation sequence

1. Present the complete architecture, reuse map, and page-composition plan.
2. Obtain explicit user approval before component-level drawing.
3. Create the root and all empty child pages, or the planned single page.
4. Export and inspect the root/single-page skeleton.
5. Populate one page at a time, reusing approved circuits and placement grammar.
6. Read back and render each completed page before continuing.
7. Run integrated connectivity and ERC from the root project.
8. Export all pages and inspect readability at normal review scale.

If a page boundary, interface, or architecture choice changes during drawing,
update the proposal and obtain approval rather than silently drifting.

## 7. Architecture acceptance checklist

- Every function and external connector has exactly one owning page.
- Cross-page nets have documented sources and consumers.
- Local helper nets do not leak across pages without a reason.
- Each functional block is visibly continuous through direct local wiring;
  labels do not replace ordinary intra-block connections.
- Direct wires stay within one functional cluster; none crosses another cluster,
  wraps around the page, or spans large empty space merely to avoid a label.
- Every local labelled break has a concrete crossover, cluster-boundary, dense
  fan-out, or cross-page reason; placement was considered before introducing it.
- Equivalent supply/ground pads on dense symbols are stacked or connected to one
  compact local bus rather than expanded into a comb of repeated labels.
- Power and ground markers use the project's label convention, while any
  required `PWR_FLAG` has only the narrow ERC-source role.
- Tight power, switching, analog, and Kelvin relationships are not split.
- The root shows the system at a glance, or the approved single page remains
  readable without an unnecessary hierarchy.
- Reused circuits identify their maintained source and required adaptations.
- Every page has a concrete visual composition plan.
- Root-level ERC and rendered-page review are both planned.
