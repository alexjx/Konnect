# Conventional Schematic Layout

Read this reference before placing or wiring any schematic page. Electrical
correctness is necessary but does not make an unreadable drawing acceptable.

## 1. Allocate the page before placing parts

Treat the sheet as a finite composition, not as an empty coordinate plane. For
an A4 page, first identify the usable area inside the drawing frame and title
block. Then make a page plan before moving or adding individual symbols:

1. List every functional block and name one primary component for each block.
   This is normally the IC, module, power device, or connector that defines the
   block's purpose. A passive network may use its first functional element as
   the anchor when it has no IC.
2. Estimate each block's required bounding box from the primary symbol, its
   owned support parts, visible fields, local wiring, and labels. Do not size a
   region from symbol origins alone.
3. Partition the usable page into non-overlapping regions that follow system
   power or signal flow. Size regions according to those estimated bounding
   boxes rather than using an arbitrary equal grid.
4. Place all primary components at stable anchors inside their assigned
   regions. Inspect their transformed symbol bounds and the gaps between
   regions before placing support parts.
5. Place each support part relative to the bounding box of the primary device
   or the pin group it serves. Use the smallest readable gap that leaves room
   for fields and orthogonal wiring; do not use one universal coordinate offset
   or leave a part floating merely because space exists.

Connect and finish one region at a time. Prefer complete direct connections
inside the region while they remain short and do not cross. When a connection
must leave its region, cross an unrelated path, or create a long route, end it
with a descriptive label on a short stub. This decision is made after the
regional placement is sound, not as a substitute for placement.

Render once after placing the primary anchors and again after completing each
block. Reject a layout when blocks spill into one another, unexplained empty
space separates owned parts, or a block has no clear visual center. If the
estimated blocks do not fit legibly on A4, compact or reallocate the regions;
if that still fails, split the page rather than shrinking the drawing below
normal review readability.

Use Konnect's symbol-bound and schematic-layout inspection tools when
available. Final acceptance is based on the rendered page because numeric
origins alone do not include fields, pin names, and wire corridors.

## 2. Plan local functional clusters

Partition each page into small circuits that can be reviewed independently,
such as an input-protection path, converter power stage, feedback divider,
sensor filter, reset network, UART header, or button input. Name the main device
and supporting parts in every cluster before wiring.

Arrange the page so that:

- energy and signals normally progress left to right;
- positive supplies enter from above and ground/returns leave below where this
  makes the function clearer;
- inputs and connectors are near the left or page edge, outputs and loads near
  the right or page edge, and control devices between them;
- bypass, bootstrap, feedback, bias, pull-up/down and protection parts sit next
  to the pins they serve;
- switching/high-current paths are visibly separate from analog measurement and
  reference paths.

Do not distribute one functional cluster across the page merely to fill space.
Whitespace separates clusters; it is not a corridor for long wires.

## 3. Choose wires or labels by relationship

Use a direct wire when both endpoints belong to the same local cluster and the
route remains short and obvious. The wire should show the local circuit, not
prove that two distant objects share a net.

Use matching labels on short outward stubs when a net:

- crosses between separate functional clusters, even on the same page;
- leaves or enters a hierarchical page;
- would cross an unrelated symbol or wire;
- would require a perimeter route, a long empty-space run, or several bends;
- fans out from a dense MCU/module to functions placed elsewhere.

Move or rotate parts before introducing a label when they belong to the same
cluster. Do not draw a long wrap-around wire to preserve direct connectivity.
Do not place one label on every passive pin inside a small circuit.

## 4. Draw unambiguous local wiring

- Use the schematic connection grid consistently; prefer orthogonal segments.
- Minimize bends, crossings and junctions. Prefer T-junctions; avoid four-way
  junctions because their intent is harder to read.
- Do not run wires through symbol bodies, text, pin names or reference/value
  fields.
- Keep branches close to the node they describe. A branch that travels past an
  unrelated block should become a labelled connection.
- Align repeated parts and labels, but allow unequal spacing where it shortens
  wires or clarifies ownership.
- A rendered page must be understandable at normal whole-page review scale;
  zooming in must not be required to identify which parts form one circuit.

## 5. Place Reference and Value as part of the circuit

Reference and Value fields belong to the symbol's visual bounding box; they are
not annotations to scatter after wiring. Their content is governed by the
workspace and project design authority, not by this drawing skill.

- Keep both fields visible for ordinary fitted components. Hide only fields
  whose established symbol convention makes them non-user-facing, such as a
  power symbol's generated reference.
- Reference normally sits above the body and Value below it. When a narrow
  vertical passive has wiring above and below, use one consistent side pair—
  normally Reference to the right and Value to the left—instead of entering the
  pin corridors.
- Keep field text horizontal at normal reading scale unless a deliberate
  vertical placement is materially clearer. A rotated or mirrored symbol does
  not justify leaving its fields inverted or carrying them through the body.
- Keep fields close enough that ownership is immediate, normally one 25–50 mil
  placement step from the nearest body edge. Allow more space only to clear a
  pin name or local wire; never let a field appear to belong to a neighbor.
- Reference and Value must not overlap each other, the symbol body, pin names,
  pin numbers, labels, wires, or another component's fields.

Use `set_schematic_field_positions` for reviewed page-specific placement. It
accepts an atomic list of exact Reference/Value targets and absolute sheet
coordinates. `reset_schematic_field_positions` only restores library anchors;
it is not a layout solution when rotation, mirroring, or local wiring makes
those anchors unreadable. After placement, run `check_schematic_field_spacing`
and inspect the rendered page. Resolve every real collision; investigate audit
geometry that disagrees with the rendering rather than ignoring it silently.

## 6. Power, ground and repeated pins

Use the project's consistent power/global labels at points of use instead of
page-spanning supply or ground rails. A `PWR_FLAG` is only an ERC assertion on
an already named rail. Group all `PWR_FLAG` symbols in one compact, aligned
page-corner area. Each flag uses a short local stub ending in an explicit label
for the asserted net; use the label type appropriate to that net's scope. The
real circuit node carries the matching label. Do not place a flag directly on
an IC supply pin, converter output, or anonymous branch, and do not make the
reviewer trace a distant wire to discover which net the flag asserts.

Prefer logical symbols with positive power pins at the top, ground pins at the
bottom, inputs/control on the left, outputs on the right, and related interfaces
grouped together. When multiple physical pads are the same electrical supply or
ground domain:

1. Prefer the verified compact representation already used by a qualified
   official symbol: either a native KiCad pin stack or one visible anchor pin
   with hidden passive equivalent pins co-located at the same point.
2. For a custom symbol, either representation is acceptable when the exact pin
   contract supports it. `create_symbol` can produce the visible-anchor form:
   use identical geometry, one visible anchor, and `hidden: true` passive pins
   for the other equivalent pads. Co-location is often clearer for modules with
   many identical GND pads because it avoids a visible pin comb.
3. If the chosen symbol exposes separate visible pins, connect them to one short
   local bus and attach one rail label to that bus.
4. Never hide, omit, stack or merge pins whose datasheet functions or electrical
   domains differ.

NC pins may be omitted from a symbol only when the exact symbol/footprint
contract and project library policy allow it; otherwise retain them with their
correct no-connect type or markers.

## 7. Dense controllers and modules

For an MCU or radio module, keep only genuinely local support circuits directly
wired at the symbol: supply/decoupling, enable/reset timing, required strap
defaults, and any immediately adjacent clock/RF network. Signals going to other
functional clusters use aligned short stubs and descriptive labels.

Select or create a logical-function symbol rather than reproducing footprint
edge order. UART, I2C, SPI, analog inputs, strap pins and ordinary GPIOs should
form readable groups. Equivalent ground and supply pads should be stacked or
collected compactly as described above.

## 8. Visual acceptance before ERC

Render every changed page and reject it before ERC if any of these are true:

- a wire spans a large portion of the page without representing one continuous
  local power or signal path;
- a wire passes through or wraps around another cluster;
- related support parts are visually detached from the device they support;
- labels, pin names, references or values overlap;
- repeated supply/ground pins form a comb of labels or individual long wires;
- the intended left-to-right flow or cluster boundaries are unclear.

After the page passes visual review, run connectivity, short, orphan, overlap
and ERC checks. A clean ERC never overrides a failed visual review.
