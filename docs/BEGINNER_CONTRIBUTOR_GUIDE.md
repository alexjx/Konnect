# Understanding and Contributing to Konnect

This guide is for a developer who is new to KiCad and wants to improve the
Konnect MCP server. You do **not** need to understand circuit design or all of
KiCad before you can make useful contributions. The important first step is to
know which part of Konnect you are changing and how that part communicates with
KiCad.

## 1. The short version

Konnect translates MCP JSON-RPC tool calls from an AI client into operations on
a KiCad project. It has three main ways to do that:

| Kind of work | How Konnect does it | Does KiCad need to be open? |
|---|---|---|
| Edit a schematic | Parse and rewrite `.kicad_sch` S-expressions | No |
| Edit a PCB interactively | Send protobuf messages over KiCad 10's NNG IPC API | Yes, with a board open |
| Export or run ERC/DRC | Start `kicad-cli` as a subprocess | No editor window, but KiCad must be installed |

This distinction explains many bugs. If a schematic tool fails, inspect file
parsing, coordinates, UUIDs, and atomic writing. If a PCB tool fails, inspect
the IPC socket, protobuf request/response, units, and which board is open. If an
export fails, inspect the generated command line and the installed KiCad
version.

The main request path is:

```text
AI client
  -> MCP transport (stdio or HTTP)
  -> McpHandler
  -> ToolRouter
  -> one tool handler
  -> schematic file OR KiCad IPC OR kicad-cli
  -> CallToolResult returned to the AI client
```

## 2. Minimal KiCad vocabulary

You only need a small vocabulary to start reading this repository.

- **Project**: a directory containing related KiCad files, normally including
  `.kicad_pro`, `.kicad_sch`, and `.kicad_pcb`.
- **Schematic**: the logical circuit diagram. Symbols are connected by wires,
  labels, and power symbols. Stored in `.kicad_sch`.
- **PCB**: the physical board. Footprints, pads, tracks, vias, zones, and board
  outlines have real dimensions and layers. Stored in `.kicad_pcb`.
- **Symbol**: the schematic representation of a part, such as a resistor or
  microcontroller. A placed symbol usually has a reference (`R1`), value
  (`10k`), library ID (`Device:R`), pins, position, rotation, and UUIDs.
- **Footprint**: the physical land pattern on the PCB. It contains pads and
  graphical elements. The schematic symbol and PCB footprint are related but
  are not the same object.
- **Pin / pad**: a symbol has pins; a footprint has pads. Connectivity maps a
  logical pin to a physical pad, usually by matching their numbers.
- **Net**: a named electrical connection, such as `GND`, `+3V3`, or `SDA`.
- **Wire / track**: a wire connects schematic pins; a copper track connects PCB
  pads and vias.
- **Via**: a plated hole that connects copper between PCB layers.
- **Zone**: a filled copper area, commonly used for ground.
- **ERC / DRC**: Electrical Rules Check for schematics and Design Rules Check
  for PCBs. They find different classes of problems.
- **Reference designator**: the unique name of a placed part (`R1`, `C3`, `U2`).
- **Library ID**: the reusable symbol or footprint definition, for example
  `Device:R`.
- **S-expression**: KiCad's parenthesized text format. For example,
  `(at 100 50 90)` represents a position and rotation.
- **KIID / UUID**: KiCad uses UUIDs to identify objects. Missing, duplicate, or
  accidentally replaced UUIDs can create subtle failures or unsafe files.

Two beginner traps are especially important:

1. KiCad library-symbol coordinates and schematic-screen coordinates use
   different Y directions. Rotation and mirroring order matters.
2. The pin's `(at X Y ROT)` is its electrical connection point. Its `length`
   describes the line drawn inward toward the symbol body.

The canonical coordinate implementation and its ground-truth tests are in
`crates/konnect-sexp/src/geometry.rs` and `crates/konnect-sexp/src/schematic.rs`.
Do not duplicate that math in a tool handler.

## 3. Repository map

### `crates/konnect`: the executable and transports

Start at `crates/konnect/src/main.rs`.

It reads configuration, initializes logging, constructs `McpHandler`, and runs
the stdio transport, HTTP transport, or both. Standard output is reserved for
the MCP protocol; diagnostics must go to standard error through `tracing`.

Useful files:

- `src/main.rs`: process entry point and transport selection
- `src/config.rs`: server configuration and defaults
- `src/transport/stdio.rs`: newline-delimited JSON-RPC
- `src/transport/http.rs`: Streamable HTTP and SSE
- `src/install.rs`: local installation of bundled skills and agents
- `tests/protocol_*.rs`: transport-level behavior

### `crates/konnect-core`: MCP routing and tool behavior

This is where most bug fixes and features belong.

- `src/mcp/handler.rs`: accepts MCP methods and records every tool call
- `src/mcp/error.rs`: structured tool error types
- `src/router/mod.rs`: loaded-toolset state and dispatch lookup
- `src/router/registry.rs`: all toolsets, declared counts, and starter kit
- `src/router/meta_tools.rs`: load/unload/list toolsets and observability tools
- `src/observability.rs`: recent-call ring buffer, statistics, and JSONL log
- `src/tools/mod.rs`: `ToolDef`, `ToolContext`, the `tool!` macro, and argument helpers
- `src/tools/*.rs`: JSON schema plus handler functions for each domain

Each tool module usually has this shape:

```rust
pub fn tools() -> Vec<ToolDef> {
    vec![tool!(/* name, description, JSON schema, handler */)]
}

async fn handle_example(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    // validate -> read/query -> mutate -> verify -> return structured result
}
```

Only the `project` and `config` toolsets are loaded at startup. Meta-tools are
always visible, and an AI client can load other toolsets on demand. A tool
addition must also keep the count in `router/registry.rs` correct; a registry
test enforces this.

### `crates/konnect-sexp`: low-level KiCad text operations

This crate parses S-expressions, applies byte-range edits, performs canonical
pin geometry, extracts schematic facts, and writes files atomically.

Use it when exact file preservation or analysis of raw nodes is important.
Important files:

- `parser.rs`: raw S-expression parser
- `writer.rs`: `SexpEdit`, `apply_edits`, and `write_atomic`
- `geometry.rs`: canonical coordinate transforms
- `schematic.rs`: extraction of symbols, pins, wires, labels, and net graphs
- `tests/proptest_parser.rs`: parser fuzz-style property tests

### `crates/konnect-schematic-editor`: typed schematic model

This is the higher-level editing API. `Schematic::load()` turns supported nodes
into Rust types such as `Symbol`, `Wire`, and `Label`. Unmodelled nodes are kept
in `raw_other` so saving should not discard them. `overwrite()` performs an
atomic save.

Use this crate when a typed object mutation is clearer and safer than textual
search-and-replace. `sch_bridge.rs` in `konnect-core` shows that migration is
still in progress: some analysis code converts typed editor objects back into
the older `konnect-sexp` structures.

### `crates/konnect-ipc`: live KiCad 10 PCB communication

This crate contains protobuf definitions copied from KiCad 10 and a synchronous
NNG request/reply client.

- `proto/`: KiCad API message definitions
- `build.rs`: generates Rust protobuf types with `prost`
- `src/client.rs`: connection, request envelope, response decoding, public API
- `src/builders.rs`: request construction and unit conversion
- `src/types.rs`: simpler public Rust types used by tools
- `tests/mock_server_test.rs`: behavior against a mock NNG endpoint

KiCad represents positions in nanometres in the IPC protocol; Konnect's public
tool API generally uses millimetres. Unit conversion errors can be off by a
factor of 1,000 or 1,000,000, so they deserve explicit tests.

The NNG client is blocking. PCB tool modules call it through
`tokio::task::spawn_blocking` so one slow KiCad call does not block the async MCP
runtime. Send and receive timeouts prevent a wedged editor from hanging forever.

### Other directories

- `crates/schematic-viewer`: separate Tauri app; excluded from the root workspace
- `plugin/`: thin Python/wxPython KiCad-side settings launcher
- `packaging/`: Plugin and Content Manager package assembly and validation
- `crates/konnect/assets/`: bundled instructions for AI clients
- `examples/`: MCP configuration examples
- `docs/`: troubleshooting and contributor documentation
- `tool-directory.md`: generated/readable catalog of exposed MCP tools

## 4. Follow one MCP call through the code

When the AI sends `tools/call`:

1. `McpHandler::handle_message` parses JSON-RPC.
2. `McpHandler::dispatch` recognizes `tools/call`.
3. `execute_tool` assigns a call ID, starts timing, and identifies the owning
   toolset.
4. `dispatch_tool` checks always-visible meta-tools first.
5. It asks `ToolRouter` for a loaded `ToolDef`.
6. The tool's async handler receives JSON arguments and a shared `ToolContext`.
7. The handler returns `CallToolResult`, or an `anyhow::Error` is converted to a
   structured `handler_error`.
8. The call observer records duration, status, error kind, and payload sizes.
9. The selected transport serializes the MCP response back to the client.

If the tool exists but its toolset is not loaded, the handler returns an
actionable `toolset_not_loaded` error. Loading or unloading a toolset also sends
`notifications/tools/list_changed` to stdio and HTTP clients.

For debugging a real tool call, inspect both stderr tracing and the call log:

- Windows: `%APPDATA%\konnect\logs\calls.jsonl`
- Linux: `~/.konnect/logs/calls.jsonl`
- macOS: `~/Library/Application Support/konnect/logs/calls.jsonl`

The MCP meta-tools `get_recent_calls` and `server_stats` expose the same kind of
information without opening the file.

## 5. Understand the three backends

### A. Schematic file editing

Typical flow:

```text
tool arguments
  -> load `.kicad_sch`
  -> find a symbol/wire/label by stable identity
  -> change a typed node or apply a bounded raw edit
  -> preserve unknown nodes and existing UUIDs
  -> atomic write
  -> return exactly what changed
```

Risks to test:

- selecting the wrong repeated label or symbol
- replacing text outside the intended S-expression block
- losing unmodelled nodes during a round trip
- changing existing UUIDs or creating duplicate UUIDs
- omitting per-instance pin UUIDs or the root instance path
- applying rotation/mirroring in the wrong coordinate system
- treating visually close points as electrically connected without the right tolerance
- failing on inherited or multi-unit library symbols
- editing a child sheet when the tool intended the root sheet
- leaving a partial file after an I/O failure

Prefer a tiny fixture that demonstrates the exact KiCad construct. After an
edit, parse the result again and assert both the intended change and preservation
of unrelated content. For high-risk changes, open a temporary copy in KiCad and
run ERC or an export as an end-to-end check.

### B. PCB IPC editing

Typical flow:

```text
tool arguments in mm
  -> `spawn_blocking`
  -> `KiCadIpcClient`
  -> discover the open PCB document
  -> convert to protobuf values (often mm -> nm)
  -> NNG request/reply
  -> convert protobuf response back to public values
  -> return JSON
```

Risks to test:

- API disabled, missing socket, stale socket, or no board open
- selecting the first open board when several are open
- wrong document/container header
- incorrect protobuf `type_url`
- incorrect units, angles, layer enum, or front/back mirroring
- blocking the Tokio runtime with synchronous NNG work
- KiCad returning no response body or a non-OK status
- mutation not integrating correctly with KiCad undo/redo
- a batch operation partially succeeding

Use `mock_server_test.rs` for protocol behavior that does not need a GUI. Keep
real-KiCad tests small, isolated, and ignored or placed in the end-to-end workflow
when they cannot run reliably in CI.

### C. `kicad-cli` commands

Typical flow:

```text
tool arguments
  -> locate `kicad-cli`
  -> build a KiCad-10-compatible command
  -> run subprocess with bounded behavior
  -> inspect exit status/stdout/stderr
  -> verify expected output files
```

CLI syntax changes between KiCad releases. Do not infer a command from an older
blog post; verify it against the supported KiCad version. A successful exit is
not enough if the requested artifact was not actually created.

## 6. A safe development environment

Install stable Rust and `protoc`, then run:

```powershell
cargo check --workspace
cargo test --workspace --lib --tests
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo build --release -p konnect
```

The Tauri viewer is separate:

```powershell
Set-Location crates/schematic-viewer
cargo test
cargo build --release
```

Never experiment on your only copy of a board. A good local setup contains:

1. a tiny throwaway KiCad project committed to a private test directory;
2. a before/after Git diff for every file-editing experiment;
3. KiCad's API enabled for IPC work;
4. the PCB editor open only when testing PCB tools;
5. a known `kicad-cli` path;
6. stderr logs captured separately from MCP stdout.

For schematic work, a useful manual check is:

1. copy a minimal fixture to a temporary directory;
2. run one tool operation;
3. confirm the result parses through both the relevant Konnect parser and KiCad;
4. save it once in KiCad and inspect the diff;
5. run ERC or export a netlist/SVG;
6. confirm unrelated UUIDs and structures stayed unchanged.

## 7. How to fix a bug

Use this sequence for most issues:

1. **Classify the backend.** Is it MCP transport/routing, schematic text,
   schematic geometry, live PCB IPC, or `kicad-cli`?
2. **Capture a minimal reproduction.** Record the exact tool name, arguments,
   result/error, KiCad version, and smallest project file that reproduces it.
3. **Read the call record.** Look for the structured error kind and duration.
4. **Reduce the fixture.** Remove unrelated components until only the failing
   KiCad construct remains.
5. **Write a failing regression test first.** Put it at the lowest layer that
   demonstrates the bug.
6. **Fix the authoritative layer.** Coordinate math belongs in `geometry.rs`,
   IPC encoding in `konnect-ipc`, and routing behavior in the router—not copied
   into individual handlers.
7. **Test preservation and errors.** Verify what must not change, and verify a
   bad input returns a useful structured error rather than panicking.
8. **Run the full checks.** Format, tests, Clippy, and any necessary real-KiCad
   smoke test.
9. **Keep the commit focused.** Do not mix a version bump, formatting sweep, or
   unrelated refactor into the bug fix.

A strong bug report or pull request includes:

- expected behavior and actual behavior
- exact tool call arguments
- minimal sanitized `.kicad_sch` or `.kicad_pcb` fixture
- KiCad and Konnect versions and operating system
- relevant `calls.jsonl` record or stderr excerpt
- a regression test
- explanation of whether KiCad was open and which document was active

## 8. Best first contributions

Start with changes that do not mutate a live design.

### Good first level: tests and error messages

- add parser tests for an S-expression shape seen in a real KiCad file
- turn a vague string error into a structured error
- test a missing argument, missing file, empty response, or unloaded toolset
- improve a troubleshooting message while keeping MCP stdout clean
- add unit-conversion boundary tests
- correct tool schema descriptions or examples

### Next level: read-only tools

- fix list/query/filter behavior
- improve analysis of labels, nets, pins, or board items
- handle duplicate names by using position, UUID, or another stable selector
- add coverage for rotation, mirroring, negative coordinates, or empty projects

### Then: schematic mutations

- mutate one typed property and prove an unrelated node survives round-trip
- add support for one currently preserved-but-unmodelled node
- replace unsafe global text search with parsed-node selection
- improve validation before writing an unsafe symbol instance

### Last: live PCB mutation and protocol changes

Do these after you are comfortable reading KiCad objects and protobuf messages.
They require a mock test plus careful testing against a running supported KiCad
version. Batch and delete operations deserve special attention because partial
success is harder to recover from.

## 9. Choosing where a regression test belongs

| Symptom | First test location |
|---|---|
| Parser crashes or misreads syntax | `konnect-sexp` unit/property tests |
| Wrong pin position or connectivity | `konnect-sexp` geometry/schematic tests |
| Typed edit loses or changes nodes | `konnect-schematic-editor` integration tests |
| Tool schema, validation, or JSON result is wrong | `konnect-core` unit/integration tests |
| Toolset cannot load or count drifts | `konnect-core/src/router/mod.rs` tests |
| JSON-RPC or list-changed behavior is wrong | `konnect` protocol tests |
| Protobuf request/response is wrong | `konnect-ipc/tests/mock_server_test.rs` |
| Only real KiCad reproduces it | small ignored/end-to-end test plus documented manual check |

Test invariants, not just happy-path output. Useful invariants include:

- parse -> write -> parse is stable;
- unrelated UUIDs remain unchanged;
- generated UUIDs are present and unique;
- coordinates round-trip within an explicit tolerance;
- tool errors have `is_error: true` and the expected structured kind;
- an IPC timeout becomes an error instead of a hang;
- a declared tool count matches the actual tools;
- a failed edit does not leave a truncated destination file.

## 10. Common mistakes in this codebase

- Printing diagnostics to stdout and corrupting the stdio MCP stream.
- Editing a repeated text value without first locating its enclosing parsed node.
- Reimplementing coordinate transforms inside a handler.
- Comparing floating-point coordinates with exact equality.
- Assuming symbol pin numbers are numeric or naturally sorted.
- Confusing a symbol library definition with a placed symbol instance.
- Confusing schematic pins with PCB pads.
- Generating new UUIDs for existing nodes during a round trip.
- Forgetting inherited, multi-unit, mirrored, or rotated symbols.
- Calling blocking IPC directly from async code.
- Assuming a configured IPC socket means a board document is open.
- Assuming a CLI command that existed in KiCad 8 or 9 still exists in KiCad 10.
- Updating a tool list without its registry count and documentation.
- Testing only the JSON response without checking the resulting KiCad artifact.

## 11. A four-week learning path

Treat this as a menu, not a deadline.

### Week 1: learn the data model

- Make a tiny KiCad project with one resistor, one LED, and ground.
- Open `.kicad_sch` and `.kicad_pcb` in a text editor and identify the objects.
- Move and rotate one item in KiCad and inspect the diff.
- Read `konnect-sexp/src/geometry.rs` and its tests.
- Run the full test suite.

### Week 2: trace the MCP server

- Read `konnect/src/main.rs`, `mcp/handler.rs`, and `router/mod.rs` in that order.
- Find one tool in `tool-directory.md`, its `tool!` definition, and its handler.
- Call it through an MCP client and inspect `calls.jsonl`.
- Add a harmless validation or structured-error regression test.

### Week 3: work on read-only schematic behavior

- Read a fixture using both schematic crates.
- Trace how `sch_analysis.rs` builds connectivity.
- Add a test for a rotated symbol, repeated label, T-junction, or orphan wire.
- Fix one analysis or error-reporting issue without writing a design file.

### Week 4: make one safe mutation

- Copy a fixture to a temporary directory.
- Change one field through the typed editor.
- Assert preservation, reload the output, and run a KiCad export.
- Submit the test and fix as one focused commit.

After this, study `konnect-ipc` and a small read-only PCB query before attempting
live mutations.

## 12. Reading order

For a productive first pass, read these files in order:

1. `README.md` — what the product promises
2. this guide — beginner mental model
3. `tool-directory.md` — user-visible API surface
4. `crates/konnect/src/main.rs` — process lifecycle
5. `crates/konnect-core/src/mcp/handler.rs` — request lifecycle
6. `crates/konnect-core/src/router/mod.rs` and `registry.rs` — tool loading
7. one small `crates/konnect-core/src/tools/*.rs` module
8. `crates/konnect-schematic-editor/src/schematic/mod.rs` — typed edits
9. `crates/konnect-sexp/src/geometry.rs` and `schematic.rs` — analysis and math
10. `crates/konnect-ipc/src/client.rs` and `builders.rs` — live PCB backend
11. `DEV.md` — detailed internal reference
12. `CONTRIBUTING.md` — checks and contribution terms

You do not need to read every tool module. Choose one vertical slice and follow
it from its MCP schema to its lowest backend and tests.

## 13. Before opening a pull request

Run:

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace --lib --tests
cargo clippy --workspace -- -D warnings
```

Also:

- update `router/registry.rs` if tool counts changed;
- update `tool-directory.md` if the public tool surface changed;
- test the supported KiCad version when behavior depends on KiCad itself;
- document any manual verification that CI cannot perform;
- verify the working tree contains no generated artifacts or real project data;
- review the contributor license agreement in `CONTRIBUTING.md`.

The best first goal is not “understand all of KiCad.” It is: **understand one
tool call completely, reproduce one bug safely, and leave behind a regression
test that makes the same bug difficult to reintroduce.**
