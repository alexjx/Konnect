# Tool System

Konnect exposes always-visible meta-tools plus domain toolsets loaded on demand.
The implementation is split between `crates/konnect-core/src/mcp`, `router`, and
`tools`.

## Definitions And Context

`ToolDef` in `crates/konnect-core/src/tools/mod.rs` is the unit used for MCP
tools. It carries a public name, description, JSON input schema, and async
handler. A definition is not necessarily exposed: `DISABLED_TOOLS` retains 33
raw implementations outside discovery and dispatch, while the selected profile
decides whether raw and workflow definitions are available.

`ToolContext` carries `ServerConfig`, the shared `ToolRouter`, the call observer,
and shared integration state. Public tool names and schema fields are API; apply
the naming and compatibility rules in `docs/NAMING_CONVENTIONS.md` and
`CONTRIBUTING.md`.

## Toolsets And Meta-Tools

`crates/konnect-core/src/router/registry.rs` declares `ALL_TOOLSETS`, the starter
set, disabled raw policy, and the filtered `tools_for` mapping. Its 20 toolsets
contain 227 raw implementations, of which 194 are exposed. Registry tests check
both implementation and exposed counts.

`router/meta_tools.rs` defines the always-visible discovery, load/unload, and
observability surface. Meta-tools are outside `ALL_TOOLSETS`. `load_toolset`
accepts either one name or an array; a successful change causes
`mcp/handler.rs` to emit `notifications/tools/list_changed`.

`tools/workflow.rs::tools()` separately defines seven guarded workflow tools.
They are not a raw toolset and must not be added to `ALL_TOOLSETS` merely to
make them discoverable.

## Exposure Profiles

`exposure_profile` is immutable for a server process:

- `legacy`: exposed raw tools plus six meta-tools (18 starter, 200 full);
- `expert`: Legacy plus seven workflows (25 starter, 207 full);
- `workflow`: seven workflows plus two observability tools (9 fixed), with no
  raw router or routing meta-tools.

## Dispatch

`McpHandler` first rejects tools outside the selected profile, then dispatches
the applicable surface in this order:

1. Handle a meta-tool.
2. Handle a workflow tool.
3. Handle a loaded raw domain tool.
4. If `auto_load_toolsets` is enabled, load the owning raw toolset and retry.
5. If an exposed raw tool is not loaded, return `toolset_not_loaded`.
6. Otherwise return `unknown_tool`.

This distinction lets a client recover from an unloaded tool without confusing
it with a misspelled or removed public API.

## Argument Contracts

`mcp/handler.rs` enforces the schema's `required` list before invoking a domain
handler. Presence is only the first gate: handlers must use the typed helpers in
`tools/mod.rs` so a value of the wrong JSON type produces a structured
`invalid_argument` response.

Use the appropriate helper, including:

- `require_str`
- `require_f64`
- `require_array`
- `require_u64`
- `get_path`

Do not use an `unwrap_or` default for a schema-required argument. An explicit
empty array is a value; an omitted required array is an invalid request.

## Response And Evidence Contracts

A response must describe what the backend observed or committed, not merely echo
what the caller requested. This rule is especially important across IPC and CLI
boundaries:

- `tools/pcb_sync.rs` compares the post-commit board with the footprint shapes
  it sent and reports or refuses mismatches.
- `tools/cli.rs` derives DRC results from every category KiCad reports rather
  than treating an absent finding in one category as a clean board.
- `tools/design_review.rs` and `tools/manufacturing.rs` require DRC evidence for
  a positive verdict and distinguish "not checked" from "none found."

When a successful write cannot be trusted from the immediate return value,
prefer a bounded read-back or an independently derived result. If the evidence
is unavailable, return an incomplete/diagnostic state rather than a positive
claim.

## Structured Errors

`CallToolResult` does not provide a separate top-level structured error object.
`crates/konnect-core/src/mcp/error.rs` serializes the error detail into text
content while setting `is_error`.

Common error kinds include `toolset_not_loaded`, `unknown_tool`,
`invalid_argument`, `file_not_found`, `conflict`, and `handler_error`. Add a new
kind in `mcp/error.rs` only when callers need a stable new classification; use
`CallToolResult::error_kind` to return it.

## Documentation Coupling

When tools are implemented, exposed, disabled, or moved between surfaces,
update the owning definitions, `router/registry.rs`, `tool-directory.md`, and
the guarded count locations named by `CONTRIBUTING.md`.
`crates/konnect/tests/doc_tool_counts.rs` independently enforces implementation,
exposure, workflow, meta, profile-full, and starter counts.
