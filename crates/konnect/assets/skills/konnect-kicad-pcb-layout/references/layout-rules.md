# Common PCB placement and floorplanning rules

Apply these rules together with project-specific datasheet requirements. A
project rule or manufacturer requirement takes precedence over a generic rule.

## Mechanical and assembly

- Place mounting holes and complete fastener/washer/tool keepouts first.
- Place edge connectors with openings facing outward and enough cable, latch,
  screwdriver and hand-access space.
- Verify polarity and pin 1 against the official drawing and 3D orientation.
- Keep complete component bodies, courtyards and mating volumes inside the
  outline unless a documented edge overhang is intentional.
- Keep tall, hot or mechanically loaded parts clear of fragile small parts.
- Preserve probe access to required test points and SWD/JTAG pads.

## Functional grouping

- Give each block one obvious center or physical anchor.
- Place bypass, feedback, timing, protection and bias parts beside what they
  serve. Empty space elsewhere is not a reason to scatter them.
- Place matched pairs and repeated channels together with consistent order,
  pitch, rotation and topology.
- Orient block interfaces toward their consumers to reduce crossings.
- Make the physical placement communicate the same flow as the schematic.

## Change containment

- Treat existing intentional placement and user edits as protected state.
- Repair the smallest failing subcircuit. Do not re-place a whole functional
  block when moving one component and its immediate support parts is sufficient.
- Do not re-place the whole board when a single block is deficient.
- Capture a complete before-state and declare a changed-reference allow-list.
  Verify afterward that all references outside the list are unchanged.
- Use external staging only before formal placement begins or after explicit
  user authorization. Routing not yet started does not authorize re-staging.
- A request to review, improve, compact or fix means local modification by
  default; it does not authorize complete reconstruction.
- If a local fix conflicts with another block, stop and request an explicit
  scope expansion instead of propagating movements across the board.
- Preserve board outline, holes, fixed connectors and completed blocks during
  local work unless they are expressly named in the requested scope.

## Courtyard clearance gate

- Treat transformed courtyard non-overlap as a hard invariant for every move,
  rotation, alignment and compaction operation.
- Query `F.CrtYd` or `B.CrtYd` from the active KiCad IPC document before and
  after mutation. Include the complete footprint transform and all courtyard
  segments, arcs and polygons.
- Test each changed footprint against every nearby footprint, mounting-hole
  keepout and board edge. Require an explicit zero-collision result before
  saving or starting another batch.
- Use axis-aligned courtyard bounds only to reject obviously separated pairs.
  When bounds overlap, require exact geometry intersection/clearance testing or
  stop. A center distance is never a component-clearance measurement.
- If a footprint has no courtyard, use verified fabrication/body geometry plus
  the documented assembly margin and report the missing courtyard. Do not
  silently substitute pad bounds, reference text or a guessed package size.
- If Konnect cannot expose live courtyard/body geometry, stop placement and fix
  the tool interface. Visual inspection remains mandatory but does not replace
  the geometry gate.

## Supply and return paths

- Minimize high-di/dt loop area, not only trace length.
- Put bypass capacitors between supply entry and device pin with an adjacent,
  low-inductance ground return.
- Keep bulk capacitors local to the load region without displacing high-frequency
  bypass capacitors.
- Reserve uninterrupted planes and copper neck widths before filling whitespace.
- Keep heater, motor and switching returns out of USB, clock, analog and bus
  reference regions.
- Use Kelvin topology where current sensing, feedback or precision references
  require it.

## Copper zones and mounting holes

- For an ordinary board-wide GND pour, create a normal zone on each requested
  copper layer and let KiCad's clearance engine avoid pads, NPTH holes and
  existing copper keepouts. Do not construct custom holes in the zone polygon
  merely because a mounting hole exists.
- Distinguish courtyard from copper keepout. A courtyard controls component
  placement but does not by itself guarantee a copper-free area. When the
  project specifies a screw-head or mounting-hole copper clearance, encode it
  in the mounting-hole footprint or an explicit board keepout, then inspect the
  filled zone to confirm the required clearance.
- Preserve project-specific power zones, priorities and current corridors when
  adding board-wide GND zones. A GND plane must not erase or obstruct required
  power copper.
- Before adding a zone, check whether the same net/layer zone already exists.
  After creation, wait for KiCad's refill to finish, verify exactly one intended
  zone per layer, inspect hole/edge avoidance, and run DRC. Do not treat a
  successful create response alone as proof of correct filled copper.
- If KiCad reports `AS_BUSY`, first cancel any active selection, move, routing
  command or modal tool and retry after the editor becomes idle. Do not close
  the formal PCB Editor or use a direct-file fallback.

## High-speed, clock and bus paths

- Place connector protection in the direct signal path with the shortest surge
  return to the reference plane.
- Put source-series termination close to the driving pin unless the datasheet
  explicitly specifies another location.
- Keep differential members paired, topologically symmetric and free of stubs.
- Keep crystal and oscillator loops extremely local, symmetric and isolated;
  do not route unrelated nets or plane splits beneath them.
- Place bus termination at the physical electrical endpoint, not wherever space
  is convenient.
- Avoid forcing a high-speed route through a dense pin field that could have
  been prevented by rotating or relocating the block.

## Thermal and high current

- Reserve copper spreading, thermal vias and airflow before compacting.
- Keep heat sources away from temperature-sensitive references and connectors
  whose plastic temperature rating is limiting.
- Prevent connector pads, vias or plane splits from creating hidden current
  bottlenecks.
- Keep parallel-layer current sharing practical with adequate stitching at
  entries, exits and neck-downs.
- Leave space for measurement and the documented full-load thermal test.

## Routing feasibility

- Inspect pad escape before closing the space around fine-pitch devices.
- Reserve routing channels for dense buses and avoid avoidable ratline crossings.
- Prefer few, short layer changes but do not sacrifice a continuous reference
  plane merely to avoid a via.
- Keep vias out of pads unless the fabrication process explicitly supports the
  required via-in-pad treatment.
- Do not allow a compact placement to make the documented trace width,
  clearance, impedance or via-count requirement impossible.

## Compactness

- Minimize the bounding box of each validated functional block.
- Use consistent courtyard-to-courtyard gaps based on assembly needs.
- Close large gaps by moving whole blocks, not by separating local support
  components from their pins.
- Align repeated rows/columns and use a deliberate pitch.
- Require a stated purpose for every large empty region: routing, copper,
  thermal, keepout, isolation or access.
- Remove dead whitespace only after preserving required corridors and planes.
- Derive the final outline from validated placement bounds plus mechanical edge
  clearance. Never use outline size as the primary compactness metric.
- On an existing board, remove local dead space without globally recomputing
  every position. Move whole validated blocks only during an explicitly
  authorized board-wide compaction pass.

## Visual and API verification

- Verify actual bodies and courtyards; an origin is not a bounding box.
- Require a live zero-courtyard-collision result for every changed footprint;
  generic DRC success and origin-distance checks are not substitutes.
- Treat asymmetric connector origins as expected and measure both sides.
- After move or rotation, verify pads and graphics changed with the footprint.
- Inspect the visible PCB Editor after every batch; do not trust a background
  IPC response as proof of visible correctness.
- Reject overlaps and board-edge violations even if a generic checker reports
  zero issues.

## Final placement checklist

- All design-document requirements have an explicit pass/fail result.
- All staged components are inside the final outline.
- No body/courtyard/keepout overlap remains.
- Every connector has correct orientation, polarity and access.
- Every bypass, clock, reset, feedback and protection component is local.
- High-speed topology, bus order and termination locations are physically clear.
- Power/current/thermal corridors remain routable at required dimensions.
- Repeated channels are ordered, aligned and consistently oriented.
- Unjustified whitespace has been removed.
- DRC and final rendered visual review expose no placement defect.
