<a name="top"></a>

<div align="center">

<img src="resources/images/KiCAD-MCP-Server-rust.svg" alt="Konnect logo" height="220" />

# Konnect

**A Codex-tailored MCP server for AI-assisted KiCad 10 workflows.**

</div>

> **Status: beta.** Review generated changes in KiCad and keep verified project
> backups. The guarded workflow reduces risk, but it does not replace electrical,
> mechanical, or manufacturing review.

Konnect connects Codex and other Model Context Protocol clients to KiCad for
schematic work, live PCB editing, verification, and production-file generation.
It is implemented in Rust and uses KiCad 10's IPC API for live board operations.

## About this fork

This is a personal fork of [mixelpixx/Konnect](https://github.com/mixelpixx/Konnect),
which grew from the earlier
[KiCAD-MCP-Server](https://github.com/mixelpixx/KiCAD-MCP-Server) project. The
upstream architecture and contributors made this work possible.

This fork has been tailored with Codex to suit my own hardware-design needs. Its
main priorities are:

- guarded, inspect-plan-apply-verify editing workflows;
- concise Codex skills that encode only necessary operating guidance;
- evidence-backed schematic, package, and PCB layout review;
- reliable live KiCad IPC operations with explicit board identity; and
- practical workflows for two-layer boards and power-electronics designs.

Fork-specific behavior and priorities may diverge from upstream. Upstream credit
does not imply endorsement of these changes.

## Capabilities

- Inspect KiCad projects, schematics, boards, components, nets, and configuration.
- Create and edit schematics, symbols, wiring, labels, fields, and hierarchy.
- Place, move, rotate, route, and inspect PCB objects through live KiCad IPC.
- Work with board outlines, zones, vias, net classes, references, and fabrication
  constraints.
- Run ERC, DRC, connectivity checks, design reviews, and targeted verification.
- Export Gerbers, drill files, position files, BOM data, PDFs, and supported 3D or
  interchange formats.
- Record tool-call diagnostics through recent-call reports, server statistics,
  and JSONL logs.

The current MCP catalog is documented in [tool-directory.md](tool-directory.md).

## Workflow profiles

Konnect exposes one process-wide capability profile:

| Profile | Intended use | Surface |
| --- | --- | --- |
| `workflow` | Guarded changes | Typed inspect, plan, apply, verify, recovery, and observability tools only |
| `expert` | Migration or unsupported operations | Guarded workflows plus on-demand raw toolsets |
| `legacy` | Compatibility | Historical starter tools and on-demand raw toolsets; guarded workflows are hidden |

`legacy` remains the default for compatibility. This fork recommends `workflow`
when the requested edit is supported. A workflow change set binds the exact file
or live board, records an allow-list and fingerprint, rejects stale plans, and
requires verification after applying.

The guarded workflow is intentionally narrower than the raw tool surface. It
currently covers selected existing-component schematic edits and moves, plus
absolute PCB footprint transforms. Use `expert` only when the requested operation
is unsupported and the broader capability is justified. Change sets are stored in
the server process and normally expire after 30 minutes; restarting the server
invalidates them.

Minimal `settings.json`:

```json
{
  "exposure_profile": "workflow",
  "transport": "stdio",
  "log_level": "info"
}
```

Configuration can also specify `kicad_cli`, `kicad_binary`, `project_dir`,
`ipc_address`, `http_address`, and `jlcpcb_db_path`. Konnect accepts JSON or TOML.
Use `--config <path>` to select an explicit file; otherwise it checks
`konnect.toml`, `settings.json`, and the platform configuration directory.

## Bundled Codex skills

Running `konnect init` installs these skills under `$CODEX_HOME/skills`, falling
back to `~/.codex/skills`:

- `konnect-kicad-schematic` — guarded schematic editing and verification;
- `konnect-kicad-pcb-layout` — live PCB modification and verification; and
- `kicad-layout-review` — read-only general, buck-converter, datasheet, and
  two-layer ground-return review.

The skills separate review from mutation. A layout audit does not authorize board
changes, and a successful tool call alone is not treated as verification.

The repository also contains `kicad-package-audit`. It is maintained separately
and is not installed by `konnect init`.

## How it works

| Area | Implementation |
| --- | --- |
| Schematic editing | Native `.kicad_sch` S-expression model with atomic file replacement |
| PCB editing | KiCad 10 IPC API over NNG/protobuf; live, board-bound, and undo-aware |
| Checks and exports | `kicad-cli` subprocesses for ERC, DRC, PDF, fabrication, and interchange outputs |
| MCP transport | JSON-RPC over stdio by default; Streamable HTTP or both are configurable |
| Tool exposure | Small starter surface, guarded workflow profile, or on-demand raw toolsets |

Schematic file operations do not require KiCad to be open. Live PCB operations
require the exact target board to be open in KiCad with IPC available.

## Requirements

Runtime requirements:

- KiCad 10;
- `kicad-cli` for CLI-backed checks and exports; and
- KiCad IPC enabled with the target board open for live PCB operations.

Source builds additionally require a Rust toolchain and `protoc` for protobuf
code generation.

Workspace CI covers Windows, Linux, and macOS, and the release workflow defines
standalone builds for those platforms. KiCad IPC, packaging, and viewer behavior
can still vary by platform, so verify the integration you intend to deploy.

## Build and install

Clone this fork and build the release binary:

```bash
git clone https://github.com/alexjx/Konnect.git
cd Konnect
cargo build --release -p konnect
```

If your GitHub SSH key is configured, `git@github.com:alexjx/Konnect.git` is an
equivalent clone URL.

The executable is written to `target/release/konnect` or
`target/release/konnect.exe` on Windows. Run the test suite before deploying a
modified build:

```bash
cargo test --workspace
```

Install or refresh the bundled Codex skills:

```powershell
# Windows
target\release\konnect.exe init
target\release\konnect.exe status
```

```bash
# Linux or macOS
./target/release/konnect init
./target/release/konnect status
```

### Register with Codex

Add the server to the Codex configuration. Using the name `kicad` matches the
bundled skill dependency metadata:

```toml
[mcp_servers.kicad]
enabled = true
command = 'D:\path\to\Konnect\target\release\konnect.exe'
args = ['--config', 'D:\path\to\konnect-settings.json']
cwd = 'D:\path\to\Konnect'
startup_timeout_sec = 120
```

Restart Codex after changing MCP configuration or installing skills. Other MCP
clients can launch the same executable over stdio with an equivalent command and
argument configuration.

### KiCad plugin package

The `konnect` crate also builds a plugin library. KiCad Plugin and Content Manager
packages are produced by the scripts under [packaging](packaging). Use a package
published by this fork when available, or build one from the checked-out revision;
do not assume an upstream package contains this fork's changes. The current PCM
metadata still uses the inherited `com.github.mixelpixx.konnect` identifier, so
review that metadata before publishing a fork-specific package.

## Schematic viewer

The optional viewer is built separately from the main workspace:

```bash
cd crates/schematic-viewer
cargo build --release
```

Then run the built executable:

```powershell
# Windows, from crates/schematic-viewer
target\release\schematic-viewer.exe path\to\root_schematic.kicad_sch
```

```bash
# Linux or macOS, from crates/schematic-viewer
./target/release/schematic-viewer path/to/root_schematic.kicad_sch
```

It watches hierarchical schematic files, renders from temporary snapshots, and
refreshes changed sheets without taking ownership of the source files. See
[DEV.md](DEV.md) for build details.

## Safety and limitations

- Confirm the exact project, schematic, or live board before every write.
- Treat project requirements and exact manufacturer documentation as design
  authority; do not invent electrical, mechanical, or fabrication limits.
- DRC and courtyard checks cover only their stated rules. They do not prove
  signal integrity, thermal performance, manufacturability, or release readiness.
- Inspect the observed state after errors or ambiguous outcomes instead of
  retrying a mutation blindly.
- Fabrication-file generation does not authorize ordering or transmitting a
  board package.

## Documentation

- [Tool directory](tool-directory.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [Developer guide](DEV.md)
- [Beginner contributor guide](docs/BEGINNER_CONTRIBUTOR_GUIDE.md)
- [Roadmap](ROADMAP.md)
- [Contributing](CONTRIBUTING.md)

## License and upstream

This repository is licensed under the [GNU AGPL-3.0-only](LICENSE). Review the
license itself for the applicable terms; this README is not legal advice.

Upstream projects:

- [mixelpixx/Konnect](https://github.com/mixelpixx/Konnect)
- [mixelpixx/KiCAD-MCP-Server](https://github.com/mixelpixx/KiCAD-MCP-Server)

Issues for this fork belong in
[alexjx/Konnect](https://github.com/alexjx/Konnect/issues).
