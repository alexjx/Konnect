# Package-audit configuration

Paths are relative to `component-package-audit.json`. Datasheet page numbers
are one-based and crop coordinates are normalized `[x0, y0, x1, y1]` from the
top-left.

```json
{
  "project_name": "Example",
  "schematic": "pcb/example.kicad_sch",
  "pcb": "pcb/example.kicad_pcb",
  "bom": "bom.csv",
  "design_documents": ["pcb/block-design.md"],
  "output_dir": "audit",
  "project_manual_status": "NOT SIGNED",
  "review_libraries": ["../COMPONENTS.md"],
  "parts": {
    "U1": {
      "class": "Exact order code and package",
      "status": "PENDING",
      "pinout": {
        "pdf": "datasheets/device.pdf",
        "evidence": [
          {
            "title": "Top view and pin numbers",
            "page": 12,
            "crop": [0.1, 0.2, 0.9, 0.8],
            "note": "Manufacturer top view"
          }
        ]
      },
      "datasheet_pins": { "1": "VCC", "2": "GND", "3": "OUT" },
      "physical_correspondence": [
        {
          "position": "Pin 1 / upper left",
          "datasheet": "VCC",
          "kicad": "Pad 1 / upper left",
          "result": "MATCH"
        }
      ]
    },
    "J3": {
      "status": "PROJECT_INTERFACE",
      "note": "Bare pogo pads; project pin contract is authoritative."
    }
  }
}
```

`pcb` is optional and supplies schematic/PCB consistency evidence only.
`output_dir` must resolve to a directory named `audit`. A `parts` key may be an
exact reference or prefix wildcard such as `Q*`; exact keys win. Use
`exclude_refs` or `PROJECT_INTERFACE` only for explicit non-component
interfaces.

## Shared reviews

`review_libraries` lists repository Markdown registries. A part selects one
with `review`; project-local fields override the shared definition. Registry
paths resolve relative to the registry file. Use the exact backticked heading
and table names expected by the parser:

```markdown
## `device-lqfp48`

### Contract

| Field | Value |
| --- | --- |
| Class | Exact order code and package |
| Status | PENDING |
| Symbol | MCU_ST_STM32G0:STM32G0B1CBTx |
| Footprint | Package_QFP:LQFP-48_7x7mm_P0.5mm |
| Datasheet | datasheets/device.pdf |
| Note | Exact order code: example |

### Pinout evidence

| Title | Page | Crop | Note |
| --- | ---: | --- | --- |
| Top view | 12 | [0.1,0.2,0.9,0.8] | Manufacturer top view |

### Datasheet pins

| Pin | Function |
| --- | --- |
| 1 | VCC |
| 2 | GND |

### Physical correspondence

| Position | Datasheet | KiCad | Result |
| --- | --- | --- | --- |
| Pin 1 / upper left | VCC | Pad 1 / upper left | MATCH |
```

Reuse is valid only for the same exact order code, symbol, footprint identifier,
library geometry, and datasheet contract. The generator does not enforce a
separate order-code field; verify it from the schematic/BOM and record it in the
review ID or note. Keep version-specific exceptions local and revisit shared
status when the contract changes. Legacy JSON registries remain readable.

## Evidence fields

- `pinout`: source PDF plus targeted evidence entries containing `title`,
  one-based `page`, normalized `crop`, and optional `note`.
- `datasheet_pins`: manufacturer physical pin number/function definitions;
  mandatory for reviewed classes with more than two electrical pins.
- `physical_correspondence`: manufacturer-versus-KiCad top-view position and
  conclusion for package-sensitive parts.

Do not configure presentation transforms to make evidence appear to match.
Manufacturer evidence and KiCad-native geometry remain independent inputs to
human review.

## Status

- `REVIEWED`: evidence was manually compared and no issue is recorded.
- `PRODUCTION_VERIFIED`: the exact contract has successful build/use sign-off.
- `SPEC_ONLY`: only a generic specification is authoritative.
- `PROJECT_INTERFACE`: project-defined pads or contacts, not a fitted part.
- `FAIL`: known pin, package, polarity, or land-pattern mismatch.
- `MISSING_EVIDENCE`: an exact part is claimed but evidence is absent.
- `PENDING`: review is incomplete.
- `project_manual_status: PASS`: explicit sign-off for one project revision;
  independent schematic/PCB findings remain visible.

Generated-report controls are review input only. They do not update the config,
shared registry, or project sign-off without explicit human acceptance.
