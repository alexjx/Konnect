# PCB routing, zones, DRC, and fabrication completion

Read this reference only when the task includes raw routing, vias, zones, DRC,
or fabrication outputs.

## Required gates

- Work against the exact live board and the project/fabricator constraints.
  Do not invent widths, clearances, via geometry, impedance, stack-up, or output
  requirements.
- Establish a baseline DRC result before broad routing or zone work so new
  findings can be distinguished from existing ones. Preserve unrelated copper
  and placement unless the user expands scope.
- After routing or zone changes, refill zones and inspect the filled result;
  zone definitions alone do not prove usable copper or connectivity.
- Re-read affected tracks, vias, nets, zones, and clearances after raw writes.
  Save through KiCad IPC only after the requested work and applicable checks
  pass.

## Release check

Run final DRC on one saved revision and resolve new errors. Record any accepted
waiver with its governing project or manufacturer basis. Generate requested
fabrication and assembly outputs from that same revision, then verify that the
expected files, layers, outline, drill treatment, and coordinate conventions
are present.

Report remaining findings and whether they block release. Generating or
reviewing outputs does not authorize ordering, uploading, or transmitting a
fabrication package; obtain explicit user authorization for that external
action.
