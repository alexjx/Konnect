---
name: konnect-kicad-symbol
description: Search, select, create, or verify KiCad schematic symbols through Konnect, including exact-part pin maps, logical pin grouping, electrical types, pin stacks, multi-unit structure, library placement, and schematic trial validation. Use when a required symbol is missing, suspect, hard to read, or being added to a shared KiCad library. Do not use for footprint geometry or ordinary schematic page layout.
---

# Konnect KiCad Symbol

Use Konnect library tools for symbol-library reads and writes. Never edit
`.kicad_sym` text directly. Symbol work does not authorize footprint edits.

A symbol is a logical interface, not a drawing of the footprint. For ordinary
ICs and modules, start from a rectangular black-box body: positive power at the
top, ground at the bottom, inputs and control on the left, and outputs on the
right. Arrange the left and right sides in visibly separated functional groups.
Bidirectional pins belong with the interface or signal-flow group they serve.
Physical pad order affects pin numbers only; it must not determine symbol-side
order or position.

Size the body from its logical content. It must enclose every pin name and
internal marking and leave functional groups visibly separated. For a new
custom symbol, two pin pitches are a useful starting margin at populated ends
and between groups, but they are not an absolute-size rejection rule for a
qualified official symbol. Judge compactness by information density,
readability at normal review scale and whether unexplained whitespace harms page
composition—not by a fixed millimetre limit or by forcing the narrowest body.

Before creating or replacing a symbol, read
[references/symbol-design.md](references/symbol-design.md) completely.

## Search before creating

Establish the exact manufacturer part number and package/module variant from the
project design authority. Then search in this order:

1. registered KiCad official libraries;
2. the component manufacturer's official KiCad library;
3. the exact symbol binding already approved in the workspace's shared KiCad
   library index, when it is not an official-library binding;
4. a reviewed shared custom symbol;
5. a new shared custom symbol only when no qualified reusable asset exists.

Do not treat a development board, related family member, distributor CAD model,
or same-package part as an exact symbol. A manufacturer's library asset is a
candidate, not proof: still compare it with the current exact-part datasheet.

## Evidence and design gate

Create a pin-contract table from the exact datasheet before drawing. For every
physical pad record the number, primary name, critical alternate function,
electrical type, supply domain, required connection/default, and whether it is
NC. Resolve exposed pads, duplicated supply pads, strapping pins and variant
differences explicitly.

Choose and record:

- single-unit or multi-unit structure;
- logical pin groups and their order;
- which physically separate pads qualify for a native pin stack;
- visible versus omittable NC pins;
- exact default footprint or intentionally blank generic footprint field.

Do not create the symbol until this contract is complete.

## Create conservatively

Use the smallest supported Konnect operation. Snapshot a shared library before a
write and search workspace references to understand impact. `create_symbol`
appends a new symbol; it is not permission to overwrite an existing qualified
name. Preserve a qualified official symbol's verified representation of
equivalent pads, including a visible anchor pin with hidden passive pins at the
same connection point. For a custom symbol with many equivalent pads, this
co-located representation is also acceptable—and normally clearer than a comb
of repeated visible pins—when every physical pad remains auditable.

`create_symbol` exposes `hidden` on every pin definition. To create the
visible-anchor form, give the anchor and every equivalent physical pin the same
`x`, `y`, `angle`, `length`, and `style`; leave the anchor visible and set
`hidden: true` plus `type: "passive"` on the other pins. The tool validates the
resolved overlap after body sizing and reports each pin's final `x`, `y`, and
`hidden` state. A standalone hidden `no_connect` pin is also supported. Do not
use hidden standalone power pins or assume hidden pins create global nets.

If another required feature cannot be represented by exposed Konnect tools,
stop and report the capability gap instead of text-editing the library.

Prefer a logical symbol: group related interfaces and leave visual separation
between groups. Package perimeter order belongs in the footprint, not the
symbol drawing.

## Verify before qualification

After creation or selection:

1. Read the symbol back with `get_symbol_info` and compare every pin against the
   pin-contract table.
2. Confirm unique physical pin numbers, electrical types, visible pin names,
   power domains, NC handling, footprint binding and metadata.
3. Place the symbol in a scratch or target schematic through Konnect, connect
   representative power, ground, interface and NC cases, and render it.
4. Run connection checks and ERC; inspect the render at normal review scale.
5. For a shared-library binding, independently audit symbol pins against
   footprint pads and the exact manufacturer land/pin documentation before
   adding it to the workspace's approved KiCad library index.

A successful append, readable preview, or clean ERC alone does not qualify a
symbol. Record the evidence, limitations and affected projects.
