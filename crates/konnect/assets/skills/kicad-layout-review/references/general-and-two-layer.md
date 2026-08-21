# General and two-layer layout checks

## General review

- Verify that placement exposes the intended functional and current flow.
  Connectors, protection, conversion, filtering, loads, and sensitive analog or
  timing blocks should form short, understandable paths rather than forcing
  long crossings or return detours.
- Check local energy storage at the pins it serves. Review the complete path
  from supply pin through the local capacitor and back to the relevant return
  pin; proximity by component origin is not proof of low loop inductance.
- Trace high-current, high-frequency, differential, clock, feedback, sense, and
  analog paths end to end. Check width and clearance transitions, vias, stubs,
  layer changes, reference continuity, coupling to aggressors, and shared
  return impedance against project or manufacturer limits. When a signal
  changes layers or reference planes, prove a nearby ground stitching via,
  return capacitor, or another short documented transition path.
- Inspect every high-current series element—pads, tracks, pours, thermal
  reliefs, vias, connectors, fuses, shunts, and necks. The weakest series
  section limits the path. Judge it using copper thickness, neck width and
  length, via construction, RMS/peak current, allowed temperature rise, and
  allowed voltage drop.
- Verify thermal flow from each heat source through permitted pads, copper,
  vias, airflow, and enclosure conditions. Do not enlarge noisy copper or use
  an exposed pad as ground unless the exact package guidance permits it.
- Check component, copper, hole, and courtyard clearance to board edges,
  cutouts, keepouts, mounting hardware, enclosure features, and assembly/tool
  access. Review polarity, pin 1, connector access, test points, and silkscreen
  legibility where they affect manufacture or service.
- Inspect final filled zones for isolated islands, narrow peninsulas, broken
  connections, unintended thermal reliefs, and clearance-created bottlenecks.
  Distinguish electrical problems from visual preferences.

## Two-layer ground and return paths

- Prefer one layer as a substantially continuous ground reference. Treat
  traces, clearances, antipads, keepouts, cutouts, and unfilled regions that
  divide it as slots, even when all surrounding copper has the same net name.
- Trace each important return from receiver or load back to its source. A fast
  signal return must remain adjacent to the outbound route and must not be
  forced around a split or slot. At a signal layer transition, require a nearby
  ground return via or another short return path proven by geometry.
- Flag a ground neck only when it lies on the required power or high-frequency
  return path and violates a documented voltage-drop, heating, current-density,
  inductance, or return-continuity criterion. A narrow-looking shape is not a
  defect when the relevant current has a demonstrated low-impedance parallel
  path or remains within documented limits.
- Measure the minimum filled width and neck length. Include thermal spokes,
  pads, and every series via transition in the bottleneck assessment. Use known
  copper thickness, plating, continuous/RMS and transient current, temperature
  rise, and allowable ground offset; label missing inputs as `NEEDS EVIDENCE`.
- Add stitching only to close a real return path: near signal layer changes,
  across an unavoidable interruption where geometry permits, or between pours
  that would otherwise become long isolated peninsulas. A via fence does not
  replace a continuous reference plane.
- Flag dead-end ground islands, long narrow peninsulas, isolated pours, and
  necks created by the final refill. Confirm that intended stitch vias connect
  to filled ground on both layers.

Before calling a neck defective, show an annotated top/bottom filled-copper
view with source, load or receiver, outbound path, actual return path, measured
neck, series vias, and any parallel path. Support power-path findings with a
resistance/voltage-drop and heating check; support fast-return findings with
return discontinuity and loop-area evidence.
