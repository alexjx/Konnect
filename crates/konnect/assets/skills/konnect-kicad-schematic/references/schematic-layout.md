# Conventional Schematic Layout

Read this reference before placing or wiring any schematic page. Electrical
correctness is necessary but does not make an unreadable drawing acceptable.

## 1. Plan local functional clusters

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

## 2. Choose wires or labels by relationship

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

## 3. Draw unambiguous local wiring

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

## 4. Power, ground and repeated pins

Use the project's consistent power/global labels at points of use instead of
page-spanning supply or ground rails. A `PWR_FLAG` is only an ERC assertion on
an already named rail.

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

## 5. Dense controllers and modules

For an MCU or radio module, keep only genuinely local support circuits directly
wired at the symbol: supply/decoupling, enable/reset timing, required strap
defaults, and any immediately adjacent clock/RF network. Signals going to other
functional clusters use aligned short stubs and descriptive labels.

Select or create a logical-function symbol rather than reproducing footprint
edge order. UART, I2C, SPI, analog inputs, strap pins and ordinary GPIOs should
form readable groups. Equivalent ground and supply pads should be stacked or
collected compactly as described above.

## 6. Visual acceptance before ERC

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
