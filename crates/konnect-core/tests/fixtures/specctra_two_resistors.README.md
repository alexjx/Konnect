# Specctra two-resistor board fixture

`specctra_two_resistors.kicad_pcb` is a deliberately small two-layer board used
to test the first fail-closed Specctra export profile. It was derived from the
repository's existing PCB integration fixture, assigned stable test UUIDs and a
closed rectangular outline, then opened and re-saved by KiCad 10.0.5 with:

```text
kicad-cli pcb upgrade --force specctra_two_resistors.kicad_pcb
```

That final KiCad-authored serialization is intentional. In particular, it
captures KiCad 10's direct `(net "NAME")` pad syntax rather than relying on a
hand-written approximation of the board format.
