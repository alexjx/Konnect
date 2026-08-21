---
name: kicad-package-audit
description: Audit every KiCad component reference against its exact device, symbol, assigned library footprint, BOM, design authority, and manufacturer evidence. Use before PCB update or release, and whenever a symbol, footprint, order code, or package contract changes.
---

# KiCad Package Audit

A matching package name or pin number is not proof of matching physical pin
position. The audit presents KiCad-native and manufacturer evidence for human
engineering review; it does not certify compatibility automatically.

## Inputs

Require a root `.kicad_sch`, BOM CSV with `Reference`, electrical design
authority, `component-package-audit.json`, and exact local datasheets where
available. A `.kicad_pcb` is optional and is used only for later consistency
checks. Read [configuration](references/configuration.md) only when creating or
updating the project map or shared review registry.

## Prepare evidence

- Let the generator export the hierarchical KiCad netlist; do not substitute
  schematic-text grepping for that source.
- Inspect each exact manufacturer PDF and configure evidence that establishes
  package identity, viewing direction, physical pin positions/functions,
  polarity, and exposed-pad behavior as applicable. This selection requires
  human engineering judgment.
- For every class with more than two numbered electrical pins, provide
  structured manufacturer `datasheet_pins`. A package-sensitive class also
  needs an explicit physical-position comparison between the manufacturer and
  KiCad top views. Two-pin classes still require polarity and package review
  when relevant.
- Group evidence only when order code, symbol, footprint identifier, and
  datasheet contract are identical. Every schematic reference must remain
  covered or be explicitly excluded as a project interface/non-component.

## Generate and review

Run:

```powershell
uv run --with pymupdf <skill>/scripts/generate_package_audit.py `
  --config <project>/component-package-audit.json
```

Do not redraw footprint evidence. The generator resolves the assigned library
footprint and uses KiCad CLI to produce native geometry.

Review both `<project>/audit/components.html` and
`<project>/audit/manifest.json`. Confirm every expected reference is covered or
excluded, evidence assets resolve, and blocking findings match the displayed
evidence. Always inspect `follow_up_findings` and schematic/PCB differences,
even when the command exits successfully or the project has explicit `PASS`.

## Status and reuse

- A multi-pin class cannot be `REVIEWED` without structured manufacturer
  pin/function evidence and an explicit human comparison. Missing or unreviewed
  evidence remains `PENDING` or `MISSING_EVIDENCE`; known mismatch is `FAIL`.
- Project `PASS` is explicit human sign-off for that revision. It may clear the
  blocking exit status but does not erase independent consistency findings.
- Reuse a shared review only for the same exact order code, symbol, footprint,
  library geometry, and datasheet evidence. The generator does not independently
  validate an order code field, so verify it from the schematic/BOM and record
  it in the review identity or note. Revisit the review when any part of the
  contract changes. Project-local fields may override shared data for that
  revision only.

## Correct findings

Report generation is read-only with respect to KiCad design sources. When an
authorized correction is required, prefer the guarded Konnect workflow for its
supported surface:

- existing schematic Value, Footprint, BOM/DNP, or existing custom fields via
  `inspect_design` -> `plan_schematic_edit` -> review -> `apply_change_set` ->
  `verify_change_set`; or
- explicit absolute live-PCB footprint transforms through the equivalent
  `plan_pcb_edit` lifecycle.

Before apply, require `planned`/`none`, the exact resource and operations, and
an allow-list containing only audited references. Reinspect and replan stale or
expired work. Correct the cause of rejected or invalid work before creating a
new plan. Stop on `effect_state: unknown` or error code `partial_apply`. Use raw
tools only when the active profile permits and the correction is outside the
typed surface, never to bypass a guarded failure. If any required mutation is
unavailable or unauthorized, stop the whole correction unless the user
explicitly accepts partial completion; in `workflow`, do not attempt
`load_toolset`.

After any correction, regenerate the audit. Keep affected classes failed or
pending until the updated evidence is reviewed.
