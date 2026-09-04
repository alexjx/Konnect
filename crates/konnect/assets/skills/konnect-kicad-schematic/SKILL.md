---
name: konnect-kicad-schematic
description: Plan, create, modify, or verify KiCad schematics through Konnect MCP. Use for reusable single-page or hierarchical architecture, symbols, wiring, component properties, placement, or electrical validation; use the guarded workflow whenever it supports the requested mutation.
---

# Konnect KiCad Schematic

Use Konnect for every schematic write. Never edit `.kicad_sch` text directly.
Do not automate the KiCad GUI unless the user explicitly requests it.

## Architecture before drawing

For a new schematic, a substantial redraw, or a flat-to-hierarchy conversion,
read [references/schematic-architecture.md](references/schematic-architecture.md)
and complete its workflow before placing, moving, or wiring component-level
symbols. A request to "start the schematic" does not by itself approve an
unstated page architecture.

For any placement or wiring work, also read
[references/schematic-layout.md](references/schematic-layout.md). Its normal
schematic grammar is an acceptance gate, not optional polish after ERC.

Start by inspecting maintained schematics, block-design documents, layout
standards, and shared application circuits already present in the workspace.
Preserve proven page composition and relative circuit grouping when the target
uses the same topology. Reuse is evidence-based: re-check values, device pins,
footprints, and project-specific constraints instead of copying them blindly.

Choose the structure that fits the design:

- A simple, cohesive circuit may use one well-planned page.
- A multi-function design normally uses a root architecture page plus complete
  functional child pages.
- For an existing project, preserve its established pinless/global-label or
  ported-sheet convention unless there is a documented reason to change it.

Before drawing, present the complete architecture: system flow; every page and
its responsibility, exclusions, connector ownership and visual composition;
all cross-page interfaces; power/ground ownership; reusable source pages; and
the verification plan. Obtain explicit user approval and record the accepted
architecture in the project design authority.

For a hierarchy, build and review the root plus all empty child pages before
populating component circuits. Then complete and render one child page at a
time. For a single-page design, review the planned functional zones before
placement. ERC success does not substitute for architecture approval or
readability at normal review scale.

## Prefer the guarded workflow

For existing-component property edits and relative component moves, use this
complete lifecycle in Expert or Workflow:

1. Call `inspect_design` with the exact schematic path. Establish the observed
   resource revision and inspect the authorized references.
2. Call `plan_schematic_edit` with the same exact path and the complete intended
   operations. Planning is zero-write.
3. Review the returned `change_set_id`, immutable operations, resource revision,
   and every structured gate. Use `get_change_set` when the retained state must
   be re-read. Do not apply a blocked, incomplete, or stale plan.
4. Call `apply_change_set` once with that `change_set_id`. Treat a refusal or
   ambiguous result as final until observed state is inspected; never create a
   replacement plan just to bypass a gate.
5. Call `verify_change_set` with the same ID. Completion requires its readback
   against the actual schematic file and a verified effect state, not merely a
   successful apply response.

If the plan is no longer wanted and has produced no effect, call
`discard_change_set`. Change sets are process-local and expire; never treat an
ID from another server process as durable authorization.

## Raw tools for unsupported operations

The guarded schematic schema currently supports existing-component edits and
relative moves. Wiring, hierarchy, symbol creation/deletion, and other
unsupported mutations require exposed raw tools in Legacy or Expert. Workflow
has no raw router; report the capability gap instead of changing profiles or
using a hidden implementation without user direction.

For a supported raw operation, load only its toolset, inspect the exact file and
targets, take a snapshot when rollback matters, invoke the smallest atomic or
batch write once, and immediately re-read the affected objects. Raw tools do
not create a stored plan or allow-list. Stop on any refusal, partial result, or
ambiguous effect. Do not apply only the supported subset of a mixed request
unless the user explicitly accepts partial completion.

## Preserve design intent

- Treat the current schematic as authoritative. Keep edits surgical and
  preserve unrelated pages, objects, annotations, and user placement. Do not
  regenerate, reflow, or normalize existing work without explicit scope.
- For new layout, arrange each functional chain in a natural left-to-right row
  that follows signal or power flow. Keep support parts near the component they
  serve; short branches and uneven spacing are acceptable when they improve
  clarity.
- Draw each *local functional cluster* as a complete connected circuit first.
  Use direct wires between a main device and the nearby parts that serve that
  exact function. Direct wiring is not a goal by itself: never stretch a wire
  across another cluster, around the page perimeter, or through large empty
  space merely to avoid a label.
- Connect separated clusters with matching labels on short outward stubs, even
  when they are on the same page. Do not replace the short, reviewable wiring
  inside a cluster with one label per pin merely because that is easier to
  generate.
- Place and, when useful, rotate parts to keep those direct local wires short,
  orthogonal and free of crossings. If a connection leaves its cluster, crosses
  an unrelated symbol/net, or needs a wrap-around route, split it into matching
  labels on short local stubs. Reposition first when the parts actually belong
  together; label the connection when they do not.
- Represent power rails and ground with the project's consistent power/global
  labels instead of drawing long rail wires through a block. A KiCad
  `PWR_FLAG`, when ERC genuinely needs an external-source declaration, is an
  ERC-only assertion attached to the already labelled rail; it is not a visual
  rail marker and never replaces the rail label.
- Prefer qualified official symbols whose pins are grouped by logical function
  rather than package edge order. For custom symbols, group interfaces together
  and put positive power at the top and ground at the bottom. Equivalent supply
  or ground pads may use a native pin stack or one visible anchor with verified
  hidden passive pins co-located at the same point. Preserve this compact
  representation in qualified official symbols. When creating a custom symbol,
  `create_symbol` can express the latter by giving the equivalent pins identical
  geometry, leaving one anchor visible, and setting `hidden: true` plus passive
  type on the others. Use a short local bus only when the chosen symbol exposes
  separate visible pins.
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

After guarded verification or raw readback, confirm unrelated content did not
change. Export changed pages when geometry or presentation changed, and run
connectivity/ERC checks when electrical connectivity could have changed. If
Konnect lacks the required surgical or verification operation, stop instead of
rebuilding the page or using an unapproved fallback.
