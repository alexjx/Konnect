<a name="top"></a>

<div align="center">

<img src="resources/images/KiCAD-MCP-Server-rust.svg" alt="KiCAD-MCP-Server Logo" height="240" />


# Konnect *BETA Release

**AI-assisted PCB design for KiCAD 10.** Konnect is a native KiCAD plugin — a single
Rust binary — that lets Claude and other AI assistants design schematics and PCBs
through the [Model Context Protocol](https://modelcontextprotocol.io) (MCP).

**193 verified raw tools across 18 on-demand toolsets, plus 7 workflow tools.** Schematic capture, PCB layout and
route planning, ERC/DRC, design-review audits, reference circuits, and a full
manufacturing export pipeline. The default `legacy` profile begins with 18 tools and
loads the relevant toolset only when needed. The opt-in `workflow` profile exposes
only 7 guarded workflows plus 2 observability tools. Two bundled Codex skills add
datasheet-driven schematic, placement, routing, review, and fabrication workflows.

> **Status: beta.** The core toolchain is tested and working, but this is a young
> release and it wants real-world mileage and review. Issues and PRs are welcome —
> see [CONTRIBUTING.md](CONTRIBUTING.md).

## Why Konnect exists

Konnect is the successor to [KiCAD-MCP-Server](https://github.com/mixelpixx/KiCAD-MCP-Server),
a Python/TypeScript project that proved AI-driven PCB design works — and, in the
process, showed exactly where that architecture runs out of road. Konnect was built
to fix those specific problems:

**The call path was too long.** In the original server, a single tool call travels
through TypeScript, schema validation, a spawned Python subprocess, JSON over
stdin/stdout, a command router, and finally SWIG-generated C++ proxy objects before
anything touches your board. That's four language and serialization boundaries, each
with its own failure modes — subprocess lifecycle management, stdout parsing that
filters out warnings KiCAD leaks into the stream, chunked-JSON reassembly. In
Konnect, a tool call is a function call. One process, one language, no plumbing.

**The dependency surface was enormous.** Running the original means carrying Node.js
and its npm tree, Python and its pip packages, wxPython, kicad-skip, and KiCAD's
SWIG bindings — two package ecosystems plus a binding layer, every one of them a
moving target that can break an install. Konnect is a single static binary, about
5 MB. There is nothing to install alongside it and nothing to version-match.

**SWIG is a dead end.** The original's PCB backend depends on KiCAD's SWIG Python
bindings, which KiCAD is deprecating in favor of its IPC API. SWIG also carried
real operational scars: a zone-fill call that can segfault the backend, proxy-object
comparison bugs, and a fallback path that can silently swap backends mid-session.
Konnect talks to KiCAD 10 through the official IPC API (protobuf over NNG) — the
interface KiCAD is investing in — with real-time board edits that integrate with
KiCAD's own undo/redo.

**Schematic edits should not corrupt files.** Konnect edits `.kicad_sch` files
through its own S-expression engine with atomic writes (write, fsync, rename), UUID
preservation, and round-trip tests — no third-party schematic library with known
gaps, no text-manipulation workarounds.

**Context economy is a feature.** Exposing ~200 tools to an LLM costs roughly 23K
tokens of context on every listing. Konnect's router loads a starter kit (~2K
tokens) and lets the model pull in toolsets on demand — plus built-in observability
(`get_recent_calls`, `server_stats`, JSONL call logs) so the model can diagnose its
own tool failures.

### Workflow-first editing

Set `exposure_profile` to `workflow` to hide raw MCP capabilities and expose a small,
typed surface: inspect, plan, inspect a change set, apply, verify, and discard. Plans
carry a file/live-document fingerprint and target allow-list; stale plans are rejected
before writing. PCB footprint moves and absolute rotations use one KiCad undo commit,
projected exact courtyard checks, and save only after verification.

Profiles are process-wide: `legacy` preserves the historical surface, `expert` adds
workflows while retaining on-demand raw toolsets, and `workflow` prevents raw dispatch
(including `load_toolset`). Configure it in `settings.json`:

```json
{ "exposure_profile": "workflow" }
```

The MVP intentionally limits schematic writes to component properties and grid-snapped
component moves, and PCB writes to footprint position/rotation. Change sets live in the
server process for 30 minutes; a restart expires them. Use `workflow` when guard enforcement
is required—`expert` deliberately allows raw tools to bypass workflow policy.

The result is smaller, faster to install, aligned with where KiCAD is going, and
built for production use rather than experimentation. The original project remains
open, maintained, and useful — see [the comparison below](#relationship-to-kicad-mcp-server).

## What it does

Instead of describing changes and applying them by hand, the AI works your project
directly:

- **Place and wire schematic components** — add resistors, ICs, connectors; wire them
  together by pin name
- **Lay out the PCB** — place, move, rotate, and route footprints in real time via
  KiCAD's IPC API, with full undo/redo integration
- **Run design checks** — ERC, DRC, connectivity validation, decoupling audits,
  power-rail review, BOM health checks
- **Export production files** — Gerbers, drill, BOM, pick-and-place, 3D models, PDF
- **Search JLCPCB parts** — find in-stock components in a local 2.5M-part catalog and
  suggest alternatives
- **Start from reference circuits** — USB-C, LDO, buck converter, STM32, I2C, LED
  templates with verified component values
- **Watch it happen** — a live schematic viewer auto-refreshes as the AI edits

### PCB reference silkscreen tools

Load the `pcb_components` toolset to inspect and edit footprint references through
KiCad IPC. `batch_set_reference_style` enforces a minimum 1.0 x 1.0 mm font with
0.15 mm stroke. `check_reference_collisions` reports same-side pad, via, board-bound,
and reference overlaps. `auto_place_references` searches compact positions around
each footprint's pad bounds. It defaults to `dry_run: true` and
`only_colliding: true`, so legal hand-placed references remain unchanged. If
enabled, only ordinary R/C references may be hidden when no legal candidate exists.

Run auto-placement in small reference or prefix groups and review the dry-run
report before setting `dry_run: false`. These tools never move footprints, pads,
vias, tracks, zones, or functional pad labels.

The full tool catalog is documented in [tool-directory.md](tool-directory.md).

## How it works

| Layer | Mechanism |
|-------|-----------|
| Schematic editing | Direct `.kicad_sch` S-expression editing with atomic writes (no KiCAD required) |
| PCB editing | KiCAD 10 IPC API (NNG + protobuf) — real-time, undo-aware, requires KiCAD running |
| Exports & checks | `kicad-cli` subprocess (Gerber, PDF, ERC, DRC, …) |
| Transport | MCP JSON-RPC over stdio (default), or Streamable HTTP (`transport = "http"` / `"both"`) |

## Installation

### From the KiCAD Plugin Manager (recommended)

1. Download `konnect-pcm-v<version>.zip` from [Releases](https://github.com/mixelpixx/Konnect/releases)
   (the `konnect-pcm-*` asset is the KiCAD plugin package; the other archives are
   standalone server binaries)
2. Open KiCAD 10 → **Plugin and Content Manager**
3. Click **Install from File** and select the zip
4. Restart KiCAD

Verify: open the **PCB Editor** → **Tools → External Plugins** → you should see
**Konnect**.

### Build from source

```bash
# protoc is required (protobuf code generation)
# Windows: choco install protoc / macOS: brew install protobuf / Linux: apt install protobuf-compiler
cargo build --release -p konnect
```

## Setup with Codex

Run the server once from a terminal or install the skills explicitly:

```powershell
konnect.exe init
```

Konnect installs `konnect-kicad-schematic` and `konnect-kicad-pcb-layout` under
`$CODEX_HOME/skills`. If `CODEX_HOME` is unset, it uses `~/.codex/skills`.
Register the same executable as the `konnect` MCP server in Codex, then restart
Codex so it discovers the skills.

## Setup with Claude Desktop

After a PCM install, the server binary lives in your KiCAD documents folder:

```
C:\Users\<YOU>\Documents\KiCad\10.0\3rdparty\plugins\com_github_mixelpixx_konnect\bin\konnect.exe
```

Edit `%APPDATA%\Claude\claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "konnect": {
      "command": "C:\\Users\\<YOU>\\Documents\\KiCad\\10.0\\3rdparty\\plugins\\com_github_mixelpixx_konnect\\bin\\konnect.exe"
    }
  }
}
```

Restart Claude Desktop and the Konnect tools appear. For Claude Code, drop the same
snippet into a `.mcp.json` in your project root (see [examples/](examples/)).

## Schematic viewer

A standalone viewer that auto-refreshes as the schematic file changes:

```bash
schematic-viewer.exe path\to\your\root_schematic.kicad_sch
```

Point it at the root sheet of a hierarchical design and every sub-sheet is rendered
too, with a depth-indented sheet selector in the toolbar. Edits saved from KiCAD (or
made by the AI through the schematic tools) re-render only the sheets that changed
and refresh the view live — rendering runs against temp-folder snapshots, so the
viewer never blocks KiCAD from saving. Pan with click-drag, zoom with the wheel,
`0` to fit, `R` to refresh, drag-and-drop to open a different file. Also launchable
by the AI via the `open_schematic_viewer` tool.

Needs the WebView2 runtime (pre-installed on Windows 10/11) and a KiCAD install for
`kicad-cli` (auto-discovered, or pass `--kicad-cli <path>`). Built separately from
the main workspace — see [DEV.md](DEV.md) for build steps.

## Requirements

- KiCAD 10 (Windows today; Linux and macOS builds are on the [roadmap](ROADMAP.md) —
  the code already compiles and passes tests on all three platforms in CI)
- `kicad-cli` (ships with KiCAD — used for exports, ERC, DRC)
- For PCB tools: KiCAD running with the target board open (IPC API)

## License: free for the little guys

Konnect is licensed under the **[GNU AGPL-3.0](LICENSE)**.

If you're a hobbyist, student, freelancer, or open-source project: **use it freely,
no strings attached.** Design boards, ship them, sell them.

If you're a business: the AGPL requires that anything you build on or around Konnect —
including software provided over a network — be open-sourced under the same license.
If that doesn't work for you, **commercial licenses are available**: see
[COMMERCIAL.md](COMMERCIAL.md).

## Relationship to KiCAD-MCP-Server

The original [Python/TypeScript project](https://github.com/mixelpixx/KiCAD-MCP-Server)
remains fully open (MIT) and maintained. Konnect is where new development happens —
the architecture it proved, rebuilt for production:

| | KiCAD-MCP-Server | Konnect |
|---|---|---|
| Runtime | Node.js + Python + SWIG bindings | Single static binary (~5 MB) |
| Tool call path | TS → subprocess → Python → SWIG C++ | Direct function call |
| PCB backend | SWIG (deprecated by KiCAD) + experimental IPC | KiCAD 10 IPC API |
| Schematic backend | kicad-skip + custom loaders | Native S-expression engine, atomic writes |
| Context cost | Router pattern | Load/unload toolsets + observability |
| Codex skills | — | 2 workflow skills bundled |
| License | MIT | AGPL-3.0 + commercial |

## Troubleshooting

**Plugin doesn't appear in KiCAD** — install via the Plugin and Content Manager (not
manual copy), then restart KiCAD.

**PCB tools return "IPC connect failed"** — open KiCAD with your board file first;
PCB tools talk to the running PCB editor.

**"kicad-cli not found"** — common install paths are auto-detected; set the path
explicitly in the plugin settings dialog or your `konnect-settings.json` if yours
is elsewhere.

## Support

- Issues & feature requests: [GitHub Issues](https://github.com/mixelpixx/Konnect/issues)
- Roadmap: [ROADMAP.md](ROADMAP.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)
- New to KiCad development: [Beginner Contributor Guide](docs/BEGINNER_CONTRIBUTOR_GUIDE.md)
