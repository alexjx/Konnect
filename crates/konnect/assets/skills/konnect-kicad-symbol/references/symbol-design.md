# KiCad Symbol Design and Review

This reference translates KiCad's library conventions into a practical Konnect
workflow. The current KiCad Library Convention remains the authority when it is
stricter than this summary.

Primary references:

- KiCad symbol requirements: <https://klc.kicad.org/symbol/>
- KiCad pin grouping: <https://klc.kicad.org/symbol/s4/s4.2/>
- KiCad native pin stacks: <https://klc.kicad.org/symbol/s4/s4.3/>
- KiCad electrical pin types: <https://klc.kicad.org/symbol/s4/s4.4/>
- KiCad hidden/NC pins: <https://klc.kicad.org/symbol/s4/s4.5/> and
  <https://klc.kicad.org/symbol/s4/s4.6/>
- KiCad schematic connectivity and ERC: <https://docs.kicad.org/9.0/en/eeschema/eeschema.html>

## 1. Establish the exact device contract

Use the manufacturer's current datasheet, package drawing and errata for the
exact orderable variant. Create one row per physical pad:

| Field | Required decision |
| --- | --- |
| Pad number | Exact footprint pad identifier, including exposed pad |
| Primary name | Datasheet name; do not silently rename a critical function |
| Alternates | Only important, common or boot-critical alternate functions |
| Electrical type | Input, output, bidirectional, passive, open collector, power input/output, or NC |
| Domain | Supply/ground domain and voltage where relevant |
| Default | Required pull, strap, enable or safe state |
| Visibility | Normal, stacked, or legitimately omitted NC |

Check variant suffixes separately. Do not infer pin compatibility from a family
name or package alone.

## 2. Reuse-source decision

Prefer, in order, an exact KiCad official symbol, the device manufacturer's
official library, and then an already qualified shared custom binding. An
existing custom binding is impact evidence, not a reason to reject a qualified
official symbol. Search by full part number and meaningful base name. Inspect candidates with
`get_symbol_info`; never select solely from a search-result name.

If a manufacturer library is maintained and compatible, register and reference
it rather than copying its symbol into a project library. Use the workspace
shared library for a custom symbol that will serve multiple projects. A project
library is reserved for truly project-specific or frozen assets.

For every candidate compare all physical pad numbers, pin names, electrical
types, NCs, exposed pads, supply domains, default footprint and datasheet link.

## 3. Logical composition

Symbols explain function; footprints explain physical geometry. The default IC
or module symbol is a rectangular black box, not a top view of the package. Do
not arrange pins around the rectangle by package-edge sequence. Physical pad
order is used only to keep the logical pin's number mapped to the correct
footprint pad.

- Put positive power pins at the top and ground/negative power at the bottom.
- Put inputs and control on the left and outputs on the right.
- Group pins by interface or purpose on the left and right sides: reset/enable,
  straps, analog, UART, I2C, SPI, USB, debug and ordinary GPIO.
- Keep each group contiguous, order its related pins consistently from top to
  bottom, and separate adjacent groups by two pin pitches measured between the
  last pin row of one group and the first pin row of the next.
- Place bidirectional pins with the interface and on the side that matches the
  most common or project-relevant signal flow; do not alternate sides merely to
  imitate package geometry.
- Keep boot/strap significance visible in the primary name or a restrained
  alternate name.
- Keep the body close to the origin while preserving the pin grid.

Use a single unit for a cohesive device. Use multiple units when independently
placeable functional banks materially improve schematics; when such units share
power pins, put shared power in a dedicated power unit. Do not split a normal
single-unit device only to hide an awkward symbol.

## 4. Pin geometry and text

- Put pin connection points outside the body so wires never need to cross the
  symbol to reach them.
- Use a 100 mil (2.54 mm) pin-origin grid and at least 100 mil pin length;
  increase length in 50 mil steps when necessary, keeping all pins consistent.
- Derive a new custom body's height from the populated pin rows. Two pin pitches
  at populated ends and between adjacent groups are a useful default that makes
  group boundaries visible; vary it when text, grouping or an official symbol's
  established composition justifies doing so.
- Derive body width from the longest visible pin names, pin-name offset and the
  reference/value fields. The body must contain all text cleanly but should not
  be widened merely to resemble the footprint, force a square, or balance empty
  space. A comparatively large official body is still compact when its area is
  used for readable names, fields and logical grouping. Compactness is relative
  to information content and page composition, not an absolute width or area.
- Keep ordinary text at 50 mil (1.27 mm). Smaller pin text is only for a genuine
  compact-geometry need.
- Use the standard 10 mil (0.254 mm) outline; fill black-box IC/module bodies
  with the schematic background color.
- Keep reference and value outside the body without overlapping pins or names.
- Use standard active-low notation; do not encode inversion twice.

## 5. Pin types, stacks and NCs

Set electrical type from the datasheet function, not from what avoids ERC:

- supplies and grounds: `power_in` or `power_out` as applicable;
- configurable MCU GPIO: `bidirectional`;
- fixed logic/control: the actual input/output/open-collector type;
- true unconnected package pads: `no_connect`.

When many physical pads are the same logical power or ground net, compress them
to one readable connection point. Two representations are acceptable:

- a KiCad native pin stack; or
- one visible anchor pin with the remaining equivalent physical pins hidden,
  passive and co-located at the same connection point.

Preserve the latter when it is used by a qualified official/manufacturer
symbol. For custom symbols, it is often preferable when it avoids a large comb
of visible GND or supply pins and remains supported by the available Konnect
operation. The pin contract must still list and verify every physical pad.

Stack only pads that the exact datasheet defines as the same electrical domain.
Do not stack separate analog/digital grounds, sense returns, exposed pads with
special rules, or multiple supply rails without explicit manufacturer evidence.

NC pins normally remain represented. They may be omitted only if they must never
be connected, including them would make the symbol unnecessarily large, and the
symbol/footprint contract still makes the physical pad count unambiguous. An NC
that the datasheet recommends tying to a potential must remain visible.

Hidden pins must not create implicit global connections. Hidden co-located pins
are allowed only when they share the visible anchor pin's connection point and
the exact datasheet proves that all represented pads are the same electrical
domain. Hidden standalone power pins remain prohibited because they can create
unexpected global nets.

## 6. Metadata and footprint contract

Use the exact device name, reference prefix, manufacturer datasheet and useful
description/keywords. A fully specified symbol must point to a valid exact
`Library:Footprint`; a generic symbol leaves the footprint blank and uses
appropriate filters.

Symbol pin number and footprint pad number form one contract. Verify both
directions:

- every connectable symbol pin maps to the intended footprint pad;
- every electrical footprint pad is represented by a normal or stacked symbol
  pin, with only justified true-NC exceptions;
- exposed pads, pad 1, polarity/orientation and manufacturer top/bottom views are
  resolved explicitly.

## 7. Konnect workflow

1. Inspect project authority, shared library index and existing references.
2. Load only the Konnect `library` toolset; search exact candidates.
3. Read the chosen candidate and build the pin-contract comparison.
4. If creation is necessary, snapshot the target shared library and add one new,
   uniquely named symbol with `create_symbol`. Existing names are refused unless
   the reviewed request explicitly sets `replace_existing: true`; replacement
   is atomic and removes legacy duplicate definitions of that exact name.
5. Read it back with `get_symbol_info`; compare all pins and properties.
6. Register the library only if needed; do not silently replace a nickname.
7. Place the symbol in a schematic, connect representative nets, render and run
   connectivity/ERC checks.
8. Complete a symbol-to-footprint package audit before recording the binding as
   qualified in the shared library index.

Konnect `create_symbol` supports basic single/multi-unit symbols and pin
visibility. Set `hidden: true` on a hidden pin. For an equivalent-pad overlap,
provide one visible anchor and one or more hidden passive pins with identical
`x`, `y`, `angle`, `length`, and `style`. Konnect validates the resolved group
after rectangular body sizing, emits KiCad `(hide yes)`, and reports the final
coordinates and visibility. `get_symbol_info` also reports `hidden` for every
pin. Standalone hidden `no_connect` pins are supported; standalone hidden power
pins remain prohibited by this workflow because their connectivity is easy to
misread.

KiCad 10 does not store a constrained per-symbol swap group for PCB pin
reassignment. When two connector contacts are physically and electrically safe
to exchange, represent both as same-name passive pins, state the intended
equivalence in maintained metadata, and let PCB pin swapping/back-annotation
choose the assignment. This never makes positive and ground interchangeable in
the finished design: the schematic net, board copper, silk, harness and physical
connector orientation must still agree.

Selecting and preserving a qualified official symbol does not require
re-authoring its native or co-located pin representation. For a new custom
symbol, treat missing native-stack, inheritance, alternate-function or
surgical-update support as a tool limitation. Do not bypass it with direct
`.kicad_sym` editing.

## 8. Acceptance checklist

- Exact part/variant and authoritative datasheet are recorded.
- Search covered workspace, KiCad official and manufacturer official libraries.
- Every physical pad is accounted for in the pin contract.
- Pin numbers, names, electrical types, domains and NC treatment match evidence.
- Pin layout is logical rather than package-perimeter order.
- The body encloses all pin names; functional groups are visually separated and
  no unexplained whitespace harms information density or page composition.
- Equivalent power/ground pads use a verified native stack or a visible anchor
  plus hidden passive co-located physical pins.
- No hidden standalone connection creates an implicit net; every hidden
  co-located pin is tied to its visible anchor and verified as the same domain.
- Symbol metadata and footprint binding are valid.
- Readback, rendered schematic trial, connectivity and ERC pass.
- Symbol-to-footprint pad audit is complete before shared qualification.
