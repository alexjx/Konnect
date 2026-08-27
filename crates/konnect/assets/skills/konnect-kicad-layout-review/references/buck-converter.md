# Buck-converter layout checks

Identify the exact topology before judging it: monolithic or external-FET,
synchronous or asynchronous, single- or multiphase, package, switching
frequency, current range, and layer count. Use the manufacturer layout section
and reference board as the governing evidence.

## Power stage and hot loops

- Identify every high-frequency switching loop in both forward and return
  paths, including planes and vias. A synchronous buck input commutation loop
  normally includes the local high-frequency input capacitor and both power
  switches; an asynchronous loop includes the capacitor, switch, and catch
  diode. Do not misidentify the lower-ripple inductor/output path as the primary
  hot loop.
- Require the high-frequency input capacitor to connect directly between the
  power VIN and power-ground terminals with minimum loop area. Nearby bulk
  capacitance does not replace the local ceramic path.
- Keep switch-node copper only as large as current, connection, and documented
  thermal needs require. Keep feedback, sense, compensation, clocks, and other
  sensitive copper away from its electric field. Do not demand a blanket void
  under the switch node or inductor unless device guidance requires it.
- Keep the inductor and output capacitors in a short, wide continuous-current
  path. Check for thermal spokes, avoidable necks, single-via bottlenecks,
  abrupt layer changes, and shared return impedance throughout the power path.

## Control, sensing, and thermal paths

- Place feedback dividers and compensation parts beside their controller pins.
  Sense the regulated output at the quiet point required by the manufacturer,
  commonly after the inductor near the output capacitor or specified load
  sense point. Route signal and return away from switch-node, gate-drive, and
  inductor hot-terminal copper.
- Follow the documented AGND/PGND connection scheme exactly. Do not invent a
  split plane, star point, or common ground convention. Keep sensitive-ground
  current out of switching and load-current paths.
- Where exposed, keep each gate-drive path and its source return short and
  tightly coupled. Place gate resistors and Kelvin-source connections as the
  controller and MOSFET guidance requires.
- Place bootstrap capacitors directly across their documented terminals with
  the smallest practical loop; apply the same rule to an external bootstrap
  diode. Do not apply bootstrap or external-gate rules to a monolithic device
  that does not expose those paths.
- Verify thermal pads, copper, and via arrays against the exact package net and
  thermal guidance. Do not assume an exposed pad is ground or enlarge the
  switch node merely for cooling.

## Avoid false positives

Judge complete three-dimensional current paths, not one layer in isolation.
Same-side placement is not universally required when a manufacturer-approved
opposite-side placement with close paired vias gives the lower-inductance
loop. Do not flag distant bulk capacitance when the local ceramic path is
correct, every large switch node without checking current and thermal needs,
or every neck without current, length, copper, and return-path evidence.

For multiphase, four-switch, current-mode, remote-sense, or other specialized
topologies, identify each phase loop, sense pair, current-sense path, and quiet
ground from the exact datasheet rather than reducing the design to this generic
checklist.
