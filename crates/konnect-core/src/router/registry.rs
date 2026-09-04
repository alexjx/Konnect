//! Static registry mapping toolset names → ToolDef slices.
//!
//! Each toolset module exposes a `tools()` function returning its Vec<ToolDef>.
//! This registry wires them together by name.

use super::ToolsetMeta;
use crate::tools::ToolDef;

/// Tool implementations retained in the binary but intentionally omitted from
/// MCP discovery and dispatch. Re-enable a tool by removing its entry here and
/// updating the matching `ALL_TOOLSETS` count.
#[derive(Debug, Clone, Copy)]
pub struct DisabledTool {
    pub name: &'static str,
    pub reason: &'static str,
}

pub static DISABLED_TOOLS: &[DisabledTool] = &[
    DisabledTool { name: "add_layer", reason: "layer-stack mutation has no verified IPC implementation" },
    DisabledTool { name: "add_net", reason: "PCB nets are schematic-derived and cannot be created through verified IPC" },
    DisabledTool { name: "assign_net_to_class", reason: "net-to-class assignment has no verified IPC implementation" },
    DisabledTool { name: "check_freerouting", reason: "diagnostic for the currently inactive autoroute workflow" },
    DisabledTool { name: "set_design_rules", reason: "board-rule mutation has no verified IPC implementation" },
    DisabledTool { name: "copy_routing_pattern", reason: "track and via cloning is not verified through IPC" },
    DisabledTool { name: "set_layer_constraints", reason: "per-layer rule mutation has no verified IPC implementation" },
    DisabledTool { name: "move_connected", reason: "use move_schematic_connection_island to preserve the complete connected group" },
    DisabledTool { name: "move_region", reason: "broad bounding-box moves conflict with surgical existing-design edits" },
    DisabledTool { name: "add_power_symbol", reason: "the schematic skill uses named global power labels and reserves power symbols for PWR_FLAG" },
    DisabledTool { name: "launch_kicad_ui", reason: "the PCB skill requires the user-controlled formal editor to be visibly open" },
    DisabledTool { name: "get_drc_violations", reason: "run_drc provides the same check with a clearer structured result" },
    DisabledTool { name: "set_board_size", reason: "use add_board_outline; it replaces the existing outline and also supports rounded corners" },
    DisabledTool { name: "add_copper_pour", reason: "use add_zone; both create a KiCad IPC copper zone, but add_zone is the canonical board operation" },
    DisabledTool { name: "export_netlist", reason: "use generate_netlist from sch_export; this legacy PCB wrapper invokes the same schematic CLI export" },
    DisabledTool { name: "audit_manufacturing", reason: "use validate_for_manufacturing for fab preflight or run_design_review for the consolidated DFM audit" },
    DisabledTool { name: "open_schematic_viewer", reason: "optional GUI helper; skills validate through exports and rendered views" },
    DisabledTool { name: "group_components", reason: "custom grouping metadata is not used by the schematic workflow" },
    DisabledTool { name: "set_active_layer", reason: "UI state is unnecessary because PCB mutations name their target layer" },
    DisabledTool { name: "import_svg_logo", reason: "decorative artwork import is a specialized manual operation" },
    DisabledTool { name: "export_dxf", reason: "specialized mechanical interchange format not used by the current fabrication workflow" },
    DisabledTool { name: "export_gencad", reason: "specialized legacy interchange format not used by the current fabrication workflow" },
    DisabledTool { name: "export_ipc2581", reason: "specialized unified output; re-enable when a fabricator explicitly requires it" },
    DisabledTool { name: "export_odb", reason: "specialized unified output; re-enable when a fabricator explicitly requires it" },
    DisabledTool { name: "delete_symbol", reason: "destructive library maintenance is outside create and verify workflows" },
    DisabledTool { name: "download_jlcpcb_database", reason: "vendor-specific database management is not part of the vendor-neutral skills" },
    DisabledTool { name: "search_jlcpcb_parts", reason: "vendor-specific sourcing is not part of the vendor-neutral skills" },
    DisabledTool { name: "get_jlcpcb_part", reason: "vendor-specific sourcing is not part of the vendor-neutral skills" },
    DisabledTool { name: "suggest_jlcpcb_alternatives", reason: "vendor-specific sourcing is not part of the vendor-neutral skills" },
    DisabledTool { name: "get_jlcpcb_database_stats", reason: "vendor-specific database diagnostics are not part of the current workflows" },
    DisabledTool { name: "enrich_datasheets", reason: "bulk LCSC mutation is superseded by exact-device datasheet verification" },
    DisabledTool { name: "save_user_config", reason: "agents may write project rules but should not silently change global preferences" },
    DisabledTool { name: "estimate_cost", reason: "vendor-specific estimates are outside fabrication-package preparation" },
];

/// Toolsets auto-loaded when the server starts.
///
/// Kept minimal so that baseline `tools/list` context stays small (18 tools
/// including meta-tools ≈ 2K tokens). The LLM expands its toolbelt on demand
/// via `load_toolset(...)`.
///
/// Starter choices:
/// - `project` — needed to open / create / save any project
/// - `config` — user preferences, design rules; call `load_user_config` at session start
pub static STARTER_KIT: &[&str] = &["project", "config"];

pub static ALL_TOOLSETS: &[ToolsetMeta] = &[
    ToolsetMeta {
        name: "project",
        description: "Create, open, save, rename, snapshot KiCAD projects, and launch the live schematic viewer",
        category: "project",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "sch_components",
        description: "Add, edit, move, rotate, and delete schematic symbols, and set the page size",
        category: "schematic",
        tool_count: 20,
    },
    ToolsetMeta {
        name: "sch_wiring",
        description: "Wires, net labels, power symbols, junctions, no-connects, pin-to-pin connections",
        category: "schematic",
        tool_count: 19,
    },
    ToolsetMeta {
        name: "sch_bus",
        description: "Buses, bus entries, and fanning a group of pins out onto a bus",
        category: "schematic",
        tool_count: 4,
    },
    ToolsetMeta {
        name: "sch_analysis",
        description: "Net connectivity, pin queries, trace paths, overlap/orphan detection",
        category: "schematic",
        tool_count: 15,
    },
    ToolsetMeta {
        name: "sch_batch",
        description: "Bulk add, edit, delete, and move schematic elements in one call",
        category: "schematic",
        tool_count: 12,
    },
    ToolsetMeta {
        name: "sch_export",
        description: "Export schematic to SVG/PDF/PNG/netlist, run ERC, and synchronize a live PCB",
        category: "schematic",
        tool_count: 10,
    },
    ToolsetMeta {
        name: "sch_hierarchy",
        description: "Hierarchical sheets: add/edit/move/delete/duplicate a sheet, hierarchy and page-numbering queries, import/add/edit/delete sheet pins, pin/label sync validation",
        category: "schematic",
        tool_count: 12,
    },
    ToolsetMeta {
        name: "pcb_board",
        description: "Board outline, layers, zones, mounting holes, board text, SVG logo import",
        category: "pcb",
        tool_count: 8,
    },
    ToolsetMeta {
        name: "pcb_components",
        description: "Place, refresh, move, rotate, flip, align, duplicate and repair PCB footprints; inspect pads; inspect and edit a placed footprint's graphics",
        category: "pcb",
        tool_count: 20,
    },
    ToolsetMeta {
        name: "pcb_routing",
        description: "Traces, via creation and dimension editing, copper pours, net classes, differential pairs, and strict Specctra SES import",
        category: "pcb",
        tool_count: 14,
    },
    ToolsetMeta {
        name: "placement",
        description: "Placement quality and automation: score with named deductions, plan decoupling rows and BGA fanouts, first placement, force-directed refinement",
        category: "pcb",
        tool_count: 5,
    },
    ToolsetMeta {
        name: "pcb_export",
        description: "Gerber, PDF, SVG, 3D model, BOM, Specctra DSN, pick-and-place, DRC, DXF/GenCAD/IPC-2581/ODB++",
        category: "pcb",
        tool_count: 8,
    },
    ToolsetMeta {
        name: "library",
        description: "Search, register, and author symbol and footprint libraries — create symbols and footprints, edit pads, graphics, metadata and 3D models",
        category: "library",
        tool_count: 17,
    },
    ToolsetMeta {
        name: "integration",
        description: "JLCPCB parts database, local Freerouting MCP routing, datasheet URLs",
        category: "integration",
        tool_count: 2,
    },
    ToolsetMeta {
        name: "verification",
        description: "DRC, design rules, layer constraints, clearance checks, KiCAD UI control (ERC is in sch_export)",
        category: "verification",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "config",
        description: "User preferences, project rules, design rules, fab constraints — call load_user_config at session start",
        category: "config",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "design_review",
        description: "AI-powered design audits: decoupling, connections, power rails, DFM, BOM health",
        category: "review",
        tool_count: 5,
    },
    ToolsetMeta {
        name: "templates",
        description: "Reference circuit library: USB-C, LDO, buck converter, STM32, I2C, LED — verified component values",
        category: "templates",
        tool_count: 4,
    },
    ToolsetMeta {
        name: "manufacturing",
        description: "Design-to-fab pipeline: export Gerber+BOM+positions package, validate for fab house, estimate cost",
        category: "manufacturing",
        tool_count: 2,
    },
];

pub fn disabled_reason(name: &str) -> Option<&'static str> {
    DISABLED_TOOLS
        .iter()
        .find(|disabled| disabled.name == name)
        .map(|disabled| disabled.reason)
}

/// Return every implemented ToolDef for registry validation.
pub fn raw_tools_for(name: &str) -> Option<Vec<ToolDef>> {
    use crate::tools::*;
    match name {
        "project" => Some(project::tools()),
        "sch_components" => Some(sch_components::tools()),
        "sch_wiring" => Some(sch_wiring::tools()),
        "sch_bus" => Some(sch_bus::tools()),
        "sch_analysis" => Some(sch_analysis::tools()),
        "sch_batch" => Some(sch_batch::tools()),
        "sch_export" => Some(sch_export::tools()),
        "sch_hierarchy" => Some(sch_hierarchy::tools()),
        "pcb_board" => Some(pcb_board::tools()),
        "pcb_components" => Some(pcb_components::tools()),
        "pcb_routing" => Some(pcb_routing::tools()),
        "placement" => Some(placement::tools()),
        "pcb_export" => Some(pcb_export::tools()),
        "library" => Some(library::tools()),
        "integration" => Some(integration::tools()),
        "verification" => Some(verification::tools()),
        "config" => Some(config::tools()),
        "design_review" => Some(design_review::tools()),
        "templates" => Some(templates::tools()),
        "manufacturing" => Some(manufacturing::tools()),
        _ => None,
    }
}

/// Return only ToolDefs currently exposed to MCP agents.
pub fn tools_for(name: &str) -> Option<Vec<ToolDef>> {
    raw_tools_for(name).map(|defs| {
        defs.into_iter()
            .filter(|def| disabled_reason(def.name).is_none())
            .collect()
    })
}
