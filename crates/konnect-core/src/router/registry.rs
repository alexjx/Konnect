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
    // Unsupported or inactive on the verified KiCad 10 IPC surface.
    DisabledTool {
        name: "add_layer",
        reason: "layer-stack mutation has no verified IPC implementation",
    },
    DisabledTool {
        name: "edit_component",
        reason: "PCB footprint field editing is query-only until IPC supports it",
    },
    DisabledTool {
        name: "add_net",
        reason: "PCB nets are schematic-derived and cannot be created through verified IPC",
    },
    DisabledTool {
        name: "assign_net_to_class",
        reason: "net-to-class assignment has no verified IPC implementation",
    },
    DisabledTool {
        name: "autoroute",
        reason: "KiCad 10 removed the CLI DSN/SES round trip required by Freerouting",
    },
    DisabledTool {
        name: "check_freerouting",
        reason: "diagnostic for the currently inactive autoroute workflow",
    },
    DisabledTool {
        name: "set_design_rules",
        reason: "board-rule mutation has no verified IPC implementation",
    },
    DisabledTool {
        name: "copy_routing_pattern",
        reason: "track and via cloning is not verified through IPC",
    },
    DisabledTool {
        name: "set_layer_constraints",
        reason: "per-layer rule mutation has no verified IPC implementation",
    },
    // Superseded by safer workflows required by the bundled Codex skills.
    DisabledTool {
        name: "move_connected",
        reason: "use move_schematic_connection_island to preserve the complete connected group",
    },
    DisabledTool {
        name: "move_region",
        reason: "broad bounding-box moves conflict with surgical existing-design edits",
    },
    DisabledTool {
        name: "add_power_symbol",
        reason: "the schematic skill uses named global power labels and reserves power symbols for PWR_FLAG",
    },
    DisabledTool {
        name: "launch_kicad_ui",
        reason: "the PCB skill requires the user-controlled formal editor to be visibly open",
    },
    DisabledTool {
        name: "get_drc_violations",
        reason: "run_drc provides the same check with a clearer structured result",
    },
    // Consolidated public workflows: retain the implementations for compatibility
    // and testing, but expose one canonical operation for each capability.
    DisabledTool {
        name: "set_board_size",
        reason: "use add_board_outline; it replaces the existing outline and also supports rounded corners",
    },
    DisabledTool {
        name: "add_copper_pour",
        reason: "use add_zone; both create a KiCad IPC copper zone, but add_zone is the canonical board operation",
    },
    DisabledTool {
        name: "export_netlist",
        reason: "use generate_netlist from sch_export; this legacy PCB wrapper invokes the same schematic CLI export",
    },
    DisabledTool {
        name: "audit_manufacturing",
        reason: "use validate_for_manufacturing for fab preflight or run_design_review for the consolidated DFM audit",
    },
    // Specialized or cosmetic operations outside the current skill workflows.
    DisabledTool {
        name: "open_schematic_viewer",
        reason: "optional GUI helper; skills validate through exports and rendered views",
    },
    DisabledTool {
        name: "group_components",
        reason: "custom grouping metadata is not used by the schematic workflow",
    },
    DisabledTool {
        name: "repair_schematic_instance_paths",
        reason: "recovery-only operation that should be enabled for a diagnosed repair",
    },
    DisabledTool {
        name: "set_active_layer",
        reason: "UI state is unnecessary because PCB mutations name their target layer",
    },
    DisabledTool {
        name: "add_board_text",
        reason: "cosmetic board authoring is outside the current placement and routing workflow",
    },
    DisabledTool {
        name: "query_board_texts",
        reason: "board-text maintenance is outside the current placement and routing workflow",
    },
    DisabledTool {
        name: "delete_board_text",
        reason: "board-text maintenance is outside the current placement and routing workflow",
    },
    DisabledTool {
        name: "import_svg_logo",
        reason: "decorative artwork import is a specialized manual operation",
    },
    DisabledTool {
        name: "add_footprint_courtyard_circle",
        reason: "live footprint courtyard patching is superseded by verified library repair",
    },
    DisabledTool {
        name: "export_dxf",
        reason: "specialized mechanical interchange format not used by the current fabrication workflow",
    },
    DisabledTool {
        name: "export_gencad",
        reason: "specialized legacy interchange format not used by the current fabrication workflow",
    },
    DisabledTool {
        name: "export_ipc2581",
        reason: "specialized unified output; re-enable when a fabricator explicitly requires it",
    },
    DisabledTool {
        name: "export_odb",
        reason: "specialized unified output; re-enable when a fabricator explicitly requires it",
    },
    DisabledTool {
        name: "delete_symbol",
        reason: "destructive library maintenance is outside create and verify workflows",
    },
    DisabledTool {
        name: "download_jlcpcb_database",
        reason: "vendor-specific database management is not part of the vendor-neutral skills",
    },
    DisabledTool {
        name: "search_jlcpcb_parts",
        reason: "vendor-specific sourcing is not part of the vendor-neutral skills",
    },
    DisabledTool {
        name: "get_jlcpcb_part",
        reason: "vendor-specific sourcing is not part of the vendor-neutral skills",
    },
    DisabledTool {
        name: "suggest_jlcpcb_alternatives",
        reason: "vendor-specific sourcing is not part of the vendor-neutral skills",
    },
    DisabledTool {
        name: "get_jlcpcb_database_stats",
        reason: "vendor-specific database diagnostics are not part of the current workflows",
    },
    DisabledTool {
        name: "enrich_datasheets",
        reason: "bulk LCSC mutation is superseded by exact-device datasheet verification",
    },
    DisabledTool {
        name: "save_user_config",
        reason: "agents may write project rules but should not silently change global preferences",
    },
    DisabledTool {
        name: "estimate_cost",
        reason: "vendor-specific estimates are outside fabrication-package preparation",
    },
];

/// Toolsets auto-loaded when the server starts.
///
/// Kept minimal so that baseline `tools/list` context stays small (~17 tools
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
        description: "Create, open, save, inspect, and snapshot KiCAD projects",
        category: "project",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "sch_components",
        description: "Add, edit, move, rotate, and delete schematic symbols",
        category: "schematic",
        tool_count: 17,
    },
    ToolsetMeta {
        name: "sch_wiring",
        description: "Wires, net labels, junctions, no-connects, and pin-to-pin connections",
        category: "schematic",
        tool_count: 20,
    },
    ToolsetMeta {
        name: "sch_analysis",
        description: "Net connectivity, pin queries, trace paths, overlap/orphan detection",
        category: "schematic",
        tool_count: 16,
    },
    ToolsetMeta {
        name: "sch_batch",
        description: "Bulk add, edit, delete, and move schematic elements in one call",
        category: "schematic",
        tool_count: 11,
    },
    ToolsetMeta {
        name: "sch_export",
        description: "Export schematic to SVG/PDF/netlist, run ERC",
        category: "schematic",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "sch_hierarchy",
        description: "Hierarchical sheets: add/edit/move/delete/duplicate and repair sheet instances, hierarchy and page-numbering queries, import/add/edit/delete sheet pins, pin/label sync validation",
        category: "schematic",
        tool_count: 13,
    },
    ToolsetMeta {
        name: "pcb_board",
        description: "Board outline, layer inspection, zones, and mounting holes",
        category: "pcb",
        tool_count: 6,
    },
    ToolsetMeta {
        name: "pcb_components",
        description: "Place, move, rotate, align, and duplicate PCB footprints",
        category: "pcb",
        tool_count: 24,
    },
    ToolsetMeta {
        name: "pcb_routing",
        description: "Traces, vias, copper pours, net classes, differential pairs",
        category: "pcb",
        tool_count: 12,
    },
    ToolsetMeta {
        name: "pcb_export",
        description: "Gerber, PDF, SVG, 3D model, BOM, netlist, pick-and-place, and zone refill",
        category: "pcb",
        tool_count: 7,
    },
    ToolsetMeta {
        name: "library",
        description: "Symbol libraries, footprint libraries, search and registration",
        category: "library",
        tool_count: 15,
    },
    ToolsetMeta {
        name: "integration",
        description: "Exact-device datasheet URL lookup",
        category: "integration",
        tool_count: 1,
    },
    ToolsetMeta {
        name: "verification",
        description: "DRC, design-rule inspection, KiCAD status, and clearance checks",
        category: "verification",
        tool_count: 4,
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
        description: "Design-to-fab pipeline: export a manufacturing package and validate readiness",
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
pub(crate) fn raw_tools_for(name: &str) -> Option<Vec<ToolDef>> {
    use crate::tools::*;
    match name {
        "project" => Some(project::tools()),
        "sch_components" => Some(sch_components::tools()),
        "sch_wiring" => Some(sch_wiring::tools()),
        "sch_analysis" => Some(sch_analysis::tools()),
        "sch_batch" => Some(sch_batch::tools()),
        "sch_export" => Some(sch_export::tools()),
        "sch_hierarchy" => Some(sch_hierarchy::tools()),
        "pcb_board" => Some(pcb_board::tools()),
        "pcb_components" => Some(pcb_components::tools()),
        "pcb_routing" => Some(pcb_routing::tools()),
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
