//! `pcb_components` toolset — place, move, rotate, query, and array footprints on the PCB.
//!
//! Most operations use the KiCAD IPC API so they integrate with KiCAD's undo/redo
//! system and don't require a separate file-sync step. `get_board_2d_view` uses
//! kicad-cli to render a PNG.

use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{get_path, require_f64, require_str, ToolContext, ToolDef};
use konnect_ipc::client::KiCadIpcClient;
use konnect_sexp::{parse_sexp, writer::find_block_with_leading_whitespace, SexpNode};
use serde_json::json;
use std::collections::HashMap;

// ─── IPC helper ───────────────────────────────────────────────────────────────

async fn with_ipc<T, F>(
    client: KiCadIpcClient,
    board: std::path::PathBuf,
    f: F,
) -> anyhow::Result<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(&KiCadIpcClient) -> anyhow::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let client = client.bind_board(board)?;
        f(&client)
    })
    .await
    {
        Ok(Ok(r)) => Ok(Ok(r)),
        Ok(Err(e)) => Ok(Err(e.to_string())),
        Err(e) => Err(anyhow::anyhow!("Thread error: {}", e)),
    }
}

macro_rules! ipc {
    ($ctx:expr, $args:expr, |$c:ident| $body:expr) => {{
        let client = $ctx.ipc.clone();
        let board = get_path($args, "board")?;
        match with_ipc(client, board, move |$c| $body).await? {
            Ok(v) => v,
            Err(msg) => {
                return Ok(CallToolResult::error(format!(
                    "KiCAD must be running with the board loaded (IPC error: {})",
                    msg
                )))
            }
        }
    }};
}

// ─── Tool definitions ─────────────────────────────────────────────────────────

pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "place_component",
            "Place a footprint on the PCB at the given position and layer via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":      { "type": "string" },
                    "footprint":  { "type": "string", "description": "Library:Footprint (e.g. 'Resistor_SMD:R_0402')" },
                    "reference":  { "type": "string", "description": "Reference designator" },
                    "x":          { "type": "number" },
                    "y":          { "type": "number" },
                    "rotation":   { "type": "number", "default": 0 },
                    "layer":      { "type": "string", "default": "F.Cu" }
                },
                "required": ["board", "footprint", "reference", "x", "y"]
            }),
            |args, ctx| async move { handle_place_component(args, ctx).await }
        ),
        tool!(
            "move_component",
            "Move a placed footprint, optionally changing its absolute rotation in the same atomic KiCad IPC commit.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" },
                    "x":         { "type": "number" },
                    "y":         { "type": "number" },
                    "rotation":  { "type": "number", "description": "Optional absolute rotation angle in degrees" }
                },
                "required": ["board", "reference", "x", "y"]
            }),
            |args, ctx| async move { handle_move_component(args, ctx).await }
        ),
        tool!(
            "rotate_component",
            "Rotate a placed footprint by atomically updating its top-level PCB transform. Close the PCB editor before calling so an in-memory board cannot overwrite the file.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" },
                    "rotation":  { "type": "number", "description": "Rotation angle in degrees" }
                },
                "required": ["board", "reference", "rotation"]
            }),
            |args, ctx| async move { handle_rotate_component(args, ctx).await }
        ),
        tool!(
            "set_component_pad_relative_angle",
            "Repair or intentionally set every pad angle in one footprint relative to the footprint body via KiCad IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":          { "type": "string" },
                    "reference":      { "type": "string" },
                    "relative_angle": { "type": "number", "description": "Pad angle relative to the footprint body in degrees" }
                },
                "required": ["board", "reference", "relative_angle"]
            }),
            |args, ctx| async move { handle_set_component_pad_relative_angle(args, ctx).await }
        ),
        tool!(
            "flip_component",
            "Flip one placed footprint between F.Cu and B.Cu using KiCad's native interactive action via IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_flip_component(args, ctx).await }
        ),
        tool!(
            "delete_component",
            "Remove a footprint from the board via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_delete_component(args, ctx).await }
        ),
        tool!(
            "edit_component",
            "Update supported properties of a placed footprint via KiCAD IPC. Assembly attributes are independently optional; omitted attributes are preserved.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" },
                    "value":     { "type": "string", "description": "New value string (currently query-only; edit in schematic and update PCB)" },
                    "exclude_from_bom": { "type": "boolean", "description": "Exclude this footprint from the production BOM (optional)" },
                    "dnp": { "type": "boolean", "description": "Mark this footprint Do Not Populate (optional)" },
                    "x": { "type": "number", "description": "Reference text X position in board mm (optional)" },
                    "y": { "type": "number", "description": "Reference text Y position in board mm (optional)" },
                    "width": { "type": "number", "description": "Reference text width in mm (optional, >= 1.0)" },
                    "height": { "type": "number", "description": "Reference text height in mm (optional, >= 1.0)" },
                    "stroke_width": { "type": "number", "description": "Reference text stroke in mm (optional, >= 0.15)" },
                    "rotation": { "type": "number", "description": "Reference text rotation in degrees (optional)" },
                    "visible": { "type": "boolean", "description": "Reference text visibility (optional)" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_edit_component(args, ctx).await }
        ),
        tool!(
            "find_component",
            "Find a footprint on the board by reference designator and return its position.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_find_component(args, ctx).await }
        ),
        tool!(
            "get_component_pads",
            "Return the pad positions and net assignments for a footprint.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_get_component_pads(args, ctx).await }
        ),
        tool!(
            "set_component_pad_nets",
            "Atomically reassign selected footprint pads to existing board nets through KiCad IPC without moving the footprint or modifying tracks.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" },
                    "pad_nets": {
                        "type": "object",
                        "description": "Map of pad number to an existing board net name, or null to clear the pad net",
                        "additionalProperties": {
                            "oneOf": [
                                { "type": "string" },
                                { "type": "null" }
                            ]
                        }
                    }
                },
                "required": ["board", "reference", "pad_nets"]
            }),
            |args, ctx| async move { handle_set_component_pad_nets(args, ctx).await }
        ),
        tool!(
            "get_component_3d_models",
            "Return embedded 3-D model filenames and transforms for a live PCB footprint.",
            json!({
                "type": "object",
                "properties": {
                    "board":     { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_get_component_3d_models(args, ctx).await }
        ),
        tool!(
            "set_component_3d_model_transform",
            "Atomically update only one embedded footprint 3-D model offset and rotation through KiCad IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":       { "type": "string" },
                    "reference":   { "type": "string" },
                    "model_index": { "type": "integer", "default": 0, "minimum": 0 },
                    "offset_x":    { "type": "number", "default": 0 },
                    "offset_y":    { "type": "number", "default": 0 },
                    "offset_z":    { "type": "number", "default": 0 },
                    "rotation_x":  { "type": "number", "default": 0 },
                    "rotation_y":  { "type": "number", "default": 0 },
                    "rotation_z":  { "type": "number", "default": 0 }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_set_component_3d_model_transform(args, ctx).await }
        ),
        tool!(
            "get_pad_position",
            "Return the schematic-space position of a specific pad number on a footprint.",
            json!({
                "type": "object",
                "properties": {
                    "board":       { "type": "string" },
                    "reference":   { "type": "string" },
                    "pad_number":  { "type": "string" }
                },
                "required": ["board", "reference", "pad_number"]
            }),
            |args, ctx| async move { handle_get_pad_position(args, ctx).await }
        ),
        tool!(
            "get_component_list",
            "List all footprints on the board with their positions, layers, and values.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_get_component_list(args, ctx).await }
        ),
        tool!(
            "get_footprint_courtyards",
            "Return transformed F.CrtYd/B.CrtYd primitives and bounding boxes from the active KiCad IPC board.",
            json!({"type":"object","properties":{"board":{"type":"string"},"references":{"type":"array","items":{"type":"string"}}},"required":["board"]}),
            |args, ctx| async move { handle_get_footprint_courtyards(args, ctx).await }
        ),
        tool!(
            "add_footprint_courtyard_circle",
            "Append a circular F.CrtYd/B.CrtYd graphic to an existing footprint through KiCad IPC.",
            json!({"type":"object","properties":{"board":{"type":"string"},"reference":{"type":"string"},"layer":{"type":"string","default":"F.CrtYd"},"diameter":{"type":"number","minimum":0.01},"line_width":{"type":"number","minimum":0.01,"default":0.05}},"required":["board","reference","diameter"]}),
            |args, ctx| async move { handle_add_footprint_courtyard_circle(args, ctx).await }
        ),
        tool!(
            "check_courtyard_overlaps",
            "Check same-side footprint courtyard bounding boxes for overlap on the active KiCad IPC board.",
            json!({"type":"object","properties":{"board":{"type":"string"},"references":{"type":"array","items":{"type":"string"},"description":"Optional refs; empty means all footprints"},"clearance":{"type":"number","minimum":0,"default":0}},"required":["board"]}),
            |args, ctx| async move { handle_check_courtyard_overlaps(args, ctx).await }
        ),
        tool!(
            "place_component_array",
            "Place multiple copies of a footprint in a grid or line array via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":        { "type": "string" },
                    "footprint":    { "type": "string" },
                    "start_x":      { "type": "number" },
                    "start_y":      { "type": "number" },
                    "count_x":      { "type": "integer", "description": "Number of columns" },
                    "count_y":      { "type": "integer", "description": "Number of rows", "default": 1 },
                    "spacing_x":    { "type": "number", "description": "Column spacing in mm" },
                    "spacing_y":    { "type": "number", "description": "Row spacing in mm", "default": 0 },
                    "ref_prefix":   { "type": "string", "description": "Reference prefix (e.g. 'R')", "default": "U" },
                    "ref_start":    { "type": "integer", "description": "Starting reference number", "default": 1 }
                },
                "required": ["board", "footprint", "start_x", "start_y", "count_x", "spacing_x"]
            }),
            |args, ctx| async move { handle_place_array(args, ctx).await }
        ),
        tool!(
            "align_components",
            "Align multiple footprints along a common X or Y axis via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":       { "type": "string" },
                    "references":  { "type": "array", "items": { "type": "string" } },
                    "axis":        { "type": "string", "description": "'x' or 'y'", "default": "x" },
                    "value":       { "type": "number", "description": "Target coordinate to align to" }
                },
                "required": ["board", "references", "value"]
            }),
            |args, ctx| async move { handle_align_components(args, ctx).await }
        ),
        tool!(
            "duplicate_component",
            "Duplicate an existing footprint at a new position via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board":         { "type": "string" },
                    "reference":     { "type": "string", "description": "Reference to duplicate" },
                    "new_reference": { "type": "string", "description": "New reference designator" },
                    "x":             { "type": "number" },
                    "y":             { "type": "number" }
                },
                "required": ["board", "reference", "new_reference", "x", "y"]
            }),
            |args, ctx| async move { handle_duplicate_component(args, ctx).await }
        ),
        tool!(
            "list_footprint_texts",
            "List footprint reference/value text position, style, layer and visibility via KiCAD IPC.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" },
                    "kind": { "type": "string", "description": "Optional: reference or value" },
                    "reference": { "type": "string", "description": "Optional exact reference filter" }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_list_footprint_texts(args, ctx).await }
        ),
        tool!(
            "edit_footprint_reference",
            "Move, rotate, show/hide, or restyle one footprint reference without moving the footprint.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" },
                    "reference": { "type": "string" },
                    "x": { "type": "number" }, "y": { "type": "number" },
                    "width": { "type": "number", "minimum": 1.0 },
                    "height": { "type": "number", "minimum": 1.0 },
                    "stroke_width": { "type": "number", "minimum": 0.15 },
                    "rotation": { "type": "number" },
                    "visible": { "type": "boolean" }
                },
                "required": ["board", "reference"]
            }),
            |args, ctx| async move { handle_edit_footprint_reference(args, ctx).await }
        ),
        tool!(
            "batch_set_reference_style",
            "Apply JLCPCB-safe reference text size/stroke to selected footprint references.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" },
                    "references": { "type": "array", "items": { "type": "string" }, "description": "Optional exact reference list; empty means all" },
                    "prefixes": { "type": "array", "items": { "type": "string" }, "description": "Optional reference prefixes such as U,J,Q" },
                    "width": { "type": "number", "minimum": 1.0, "default": 1.0 },
                    "height": { "type": "number", "minimum": 1.0, "default": 1.0 },
                    "stroke_width": { "type": "number", "minimum": 0.15, "default": 0.15 }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_batch_set_reference_style(args, ctx).await }
        ),
        tool!(
            "check_reference_collisions",
            "Check visible footprint references against pads, vias, board edges and other references.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" },
                    "copper_clearance": { "type": "number", "default": 0.20 },
                    "edge_clearance": { "type": "number", "default": 0.30 }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_check_reference_collisions(args, ctx).await }
        ),
        tool!(
            "auto_place_references",
            "Place footprint references around their own pad bounds while avoiding pads, vias, board edges and other references.",
            json!({
                "type": "object",
                "properties": {
                    "board": { "type": "string" },
                    "references": { "type": "array", "items": { "type": "string" } },
                    "prefixes": { "type": "array", "items": { "type": "string" } },
                    "width": { "type": "number", "minimum": 1.0, "default": 1.0 },
                    "height": { "type": "number", "minimum": 1.0, "default": 1.0 },
                    "stroke_width": { "type": "number", "minimum": 0.15, "default": 0.15 },
                    "copper_clearance": { "type": "number", "default": 0.20 },
                    "edge_clearance": { "type": "number", "default": 0.30 },
                    "placement_gap": { "type": "number", "default": 0.35 },
                    "only_colliding": { "type": "boolean", "default": true, "description": "Keep already legal references at their current positions" },
                    "hide_unplaced_passives": { "type": "boolean", "default": true },
                    "dry_run": { "type": "boolean", "default": true }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_auto_place_references(args, ctx).await }
        ),
        tool!(
            "get_board_2d_view",
            "Render the PCB as a 2-D image using kicad-cli and return it as a base64 PNG.",
            json!({
                "type": "object",
                "properties": {
                    "board":  { "type": "string" },
                    "layers": {
                        "type": "array",
                        "description": "Layers to include (empty = default copper + silkscreen)",
                        "items": { "type": "string" }
                    }
                },
                "required": ["board"]
            }),
            |args, ctx| async move { handle_get_board_2d_view(args, ctx).await }
        ),
    ]
}

pub fn pad_layout_tools() -> Vec<ToolDef> {
    vec![tool!(
        "clone_component_instance",
        "Clone a complete live footprint instance as a new reference while explicitly setting library identity, placement, schematic association and optional 3-D model filename.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "source_reference": { "type": "string" },
                "new_reference": { "type": "string" },
                "value": { "type": "string" },
                "footprint": { "type": "string", "description": "Library:Footprint" },
                "x": { "type": "number" }, "y": { "type": "number" },
                "rotation": { "type": "number", "default": 0 },
                "layer": { "type": "string", "default": "F.Cu" },
                "symbol_path": { "type": "string" },
                "sheet_name": { "type": "string" },
                "sheet_file": { "type": "string" },
                "model_filename": { "type": "string" },
                "exclude_from_bom": { "type": "boolean", "default": false },
                "dnp": { "type": "boolean", "default": false }
            },
            "required": ["board", "source_reference", "new_reference", "value", "footprint", "x", "y", "symbol_path", "sheet_name", "sheet_file"]
        }),
        |args, ctx| async move { handle_clone_component_instance(args, ctx).await }
    ), tool!(
        "replace_component_footprint",
        "Atomically replace one live footprint with an exact library footprint while preserving its KIID and schematic association; pad nets are assigned explicitly by pad number.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "footprint": { "type": "string", "description": "Library:Footprint" },
                "value": { "type": "string" },
                "x": { "type": "number" },
                "y": { "type": "number" },
                "rotation": { "type": "number", "default": 0 },
                "layer": { "type": "string", "default": "F.Cu" },
                "pad_nets": {
                    "type": "object",
                    "description": "Exact pad-number to existing PCB net-name mapping",
                    "additionalProperties": { "type": "string" }
                },
                "exclude_from_bom": { "type": "boolean", "default": false },
                "dnp": { "type": "boolean", "default": false }
            },
            "required": ["board", "reference", "footprint", "value", "x", "y", "pad_nets"]
        }),
        |args, ctx| async move { handle_replace_component_footprint(args, ctx).await }
    ), tool!(
        "replace_footprint_user_texts",
        "Atomically replace all non-field user texts in one live footprint with explicit board-absolute text definitions.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "texts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" },
                            "layer": { "type": "string" },
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "rotation": { "type": "number", "default": 0 },
                            "size": { "type": "number" },
                            "stroke_width": { "type": "number" }
                        },
                        "required": ["text", "layer", "x", "y", "size", "stroke_width"]
                    }
                }
            },
            "required": ["board", "reference", "texts"]
        }),
        |args, ctx| async move { handle_replace_footprint_user_texts(args, ctx).await }
    ), tool!(
        "normalize_two_pad_smd_footprint",
        "Atomically replace a live footprint's library identity and optional two-pad SMD nominal pad geometry while preserving UUIDs, placement, graphics, fields, assembly attributes, pad nets and tracks.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "library_nickname": { "type": "string" },
                "entry_name": { "type": "string" },
                "value": { "type": "string", "description": "Optional displayed footprint value; defaults to entry_name" },
                "description": { "type": "string" },
                "keywords": { "type": "string" },
                "pad_spacing": { "type": "number", "exclusiveMinimum": 0 },
                "pad_width": { "type": "number", "exclusiveMinimum": 0 },
                "pad_height": { "type": "number", "exclusiveMinimum": 0 }
                ,"pad_roundrect_ratio": { "type": "number", "minimum": 0, "maximum": 0.5 }
                ,"courtyard_half_span": { "type": "number", "exclusiveMinimum": 0 }
                ,"silk_segment_half_length": { "type": "number", "exclusiveMinimum": 0 }
            },
            "required": ["board", "reference", "library_nickname", "entry_name"]
        }),
        |args, ctx| async move { handle_normalize_two_pad_smd_footprint(args, ctx).await }
    ), tool!(
        "set_component_3d_model",
        "Replace all embedded 3-D models in one live footprint with one explicitly defined model.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "filename": { "type": "string" },
                "offset_x": { "type": "number", "default": 0 },
                "offset_y": { "type": "number", "default": 0 },
                "offset_z": { "type": "number", "default": 0 },
                "rotation_x": { "type": "number", "default": 0 },
                "rotation_y": { "type": "number", "default": 0 },
                "rotation_z": { "type": "number", "default": 0 },
                "scale_x": { "type": "number", "default": 1 },
                "scale_y": { "type": "number", "default": 1 },
                "scale_z": { "type": "number", "default": 1 }
            },
            "required": ["board", "reference", "filename"]
        }),
        |args, ctx| async move { handle_set_component_3d_model(args, ctx).await }
    ), tool!(
        "replace_component_pad_layout",
        "Atomically replace every pad in one live footprint via KiCad IPC while preserving footprint placement, graphics, fields, courtyard and existing numbered-pad nets.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "description": { "type": "string" },
                "pads": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "number": { "type": "string" },
                            "type": { "type": "string", "enum": ["thru_hole", "np_thru_hole"] },
                            "shape": { "type": "string", "enum": ["circle", "oval", "rect", "roundrect"] },
                            "x": { "type": "number", "description": "Board-absolute X in mm" },
                            "y": { "type": "number", "description": "Board-absolute Y in mm" },
                            "width": { "type": "number" },
                            "height": { "type": "number" },
                            "drill_width": { "type": "number" },
                            "drill_height": { "type": "number" },
                            "roundrect_ratio": { "type": "number", "minimum": 0, "maximum": 0.5, "description": "Optional corner radius ratio for roundrect pads" }
                        },
                        "required": ["number", "type", "shape", "x", "y", "width", "height", "drill_width", "drill_height"]
                    }
                }
            },
            "required": ["board", "reference", "pads"]
        }),
        |args, ctx| async move { handle_replace_component_pad_layout(args, ctx).await }
    ), tool!(
        "replace_footprint_graphic_segments",
        "Atomically replace all nested footprint graphic shapes on selected layers with explicit straight segments through KiCad IPC. Pads, nets, fields, user text, placement, 3-D models and unrelated layers are preserved. Coordinates are board-absolute for live KiCad 10 footprint instances.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "layers": {
                    "type": "array",
                    "minItems": 1,
                    "items": { "type": "string" }
                },
                "segments": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "layer": { "type": "string" },
                            "width": { "type": "number", "exclusiveMinimum": 0 },
                            "x1": { "type": "number" },
                            "y1": { "type": "number" },
                            "x2": { "type": "number" },
                            "y2": { "type": "number" }
                        },
                        "required": ["layer", "width", "x1", "y1", "x2", "y2"]
                    }
                }
            },
            "required": ["board", "reference", "layers", "segments"]
        }),
        |args, ctx| async move { handle_replace_footprint_graphic_segments(args, ctx).await }
    ), tool!(
        "update_footprint_mechanical_geometry",
        "Atomically replace one rectangular courtyard primitive and, when zone_points is supplied, one nested footprint zone polygon through KiCad IPC while preserving footprint identity, pads, nets, fields, placement and all unrelated graphics. Coordinates are board-absolute for live PCB footprint instances.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "zone_index": { "type": "integer", "minimum": 0 },
                "zone_points": {
                    "type": "array",
                    "description": "Polygon vertices in board-absolute coordinates for the live PCB footprint instance.",
                    "minItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" }
                        },
                        "required": ["x", "y"]
                    }
                },
                "zone_layers": {
                    "type": "array",
                    "description": "Optional replacement copper layers for the nested zone, for example ['F.Cu']. Omit to preserve existing layers.",
                    "items": { "type": "string" }
                },
                "courtyard_layer": { "type": "string", "default": "F.CrtYd" },
                "courtyard_index": { "type": "integer", "minimum": 0, "default": 0 },
                "courtyard_x1": { "type": "number", "description": "Board-absolute X coordinate of the first courtyard corner." },
                "courtyard_y1": { "type": "number", "description": "Board-absolute Y coordinate of the first courtyard corner." },
                "courtyard_x2": { "type": "number", "description": "Board-absolute X coordinate of the opposite courtyard corner." },
                "courtyard_y2": { "type": "number", "description": "Board-absolute Y coordinate of the opposite courtyard corner." }
            },
            "required": ["board", "reference", "courtyard_x1", "courtyard_y1", "courtyard_x2", "courtyard_y2"]
        }),
        |args, ctx| async move { handle_update_footprint_mechanical_geometry(args, ctx).await }
    ), tool!(
        "set_footprint_graphics_layer",
        "Atomically move all nested graphic shapes and user text on one layer of a live footprint to another layer and optionally update exact-match user texts through KiCad IPC while preserving placement, pads, nets, fields and unrelated graphics.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" },
                "from_layer": { "type": "string" },
                "to_layer": { "type": "string" },
                "text_updates": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "match_text": { "type": "string" },
                            "new_text": { "type": "string" },
                            "x": { "type": "number", "description": "Board-absolute X coordinate in mm" },
                            "y": { "type": "number", "description": "Board-absolute Y coordinate in mm" },
                            "rotation": { "type": "number" },
                            "layer": { "type": "string" }
                        },
                        "required": ["match_text"]
                    }
                }
            },
            "required": ["board", "reference", "from_layer", "to_layer"]
        }),
        |args, ctx| async move { handle_set_footprint_graphics_layer(args, ctx).await }
    ), tool!(
        "delete_footprint_nested_zones",
        "Delete every nested zone/keepout from one live PCB footprint through KiCad IPC while preserving placement, pads, nets, fields and all non-zone graphics.",
        json!({
            "type": "object",
            "properties": {
                "board": { "type": "string" },
                "reference": { "type": "string" }
            },
            "required": ["board", "reference"]
        }),
        |args, ctx| async move { handle_delete_footprint_nested_zones(args, ctx).await }
    )]
}

async fn handle_list_footprint_texts(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let kind = args["kind"].as_str().map(str::to_string);
    let reference = args["reference"].as_str().map(str::to_string);
    let texts = ipc!(ctx, args, |c| c.list_footprint_texts());
    let filtered: Vec<_> = texts
        .into_iter()
        .filter(|t| kind.as_ref().is_none_or(|k| &t.kind == k))
        .filter(|t| reference.as_ref().is_none_or(|r| &t.reference == r))
        .collect();
    Ok(CallToolResult::json(
        &json!({ "count": filtered.len(), "texts": filtered }),
    ))
}

fn optional_f64(args: &serde_json::Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

async fn handle_edit_footprint_reference(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let width = optional_f64(args, "width");
    let height = optional_f64(args, "height");
    let stroke = optional_f64(args, "stroke_width");
    let x = optional_f64(args, "x");
    let y = optional_f64(args, "y");
    let rotation = optional_f64(args, "rotation");
    let visible = args.get("visible").and_then(|v| v.as_bool());
    if width.is_some_and(|v| v < 1.0) || height.is_some_and(|v| v < 1.0) {
        return Ok(CallToolResult::error(
            "JLCPCB text width/height must be >= 1.0 mm",
        ));
    }
    if stroke.is_some_and(|v| v < 0.15) {
        return Ok(CallToolResult::error(
            "JLCPCB silkscreen stroke width must be >= 0.15 mm",
        ));
    }
    let ref_ipc = reference.clone();
    ipc!(ctx, args, |c| c.edit_reference_text(
        &ref_ipc, x, y, width, height, stroke, rotation, visible
    ));
    Ok(CallToolResult::json(&json!({ "updated": reference })))
}

async fn handle_batch_set_reference_style(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let width = optional_f64(args, "width").unwrap_or(1.0);
    let height = optional_f64(args, "height").unwrap_or(1.0);
    let stroke = optional_f64(args, "stroke_width").unwrap_or(0.15);
    if width < 1.0 || height < 1.0 || stroke < 0.15 {
        return Ok(CallToolResult::error(
            "JLCPCB minimum is 1.0 x 1.0 mm with 0.15 mm stroke",
        ));
    }
    let requested: Vec<String> = args["references"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let prefixes: Vec<String> = args["prefixes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let texts = ipc!(ctx, args, |c| c.list_footprint_texts());
    let refs: Vec<String> = texts
        .into_iter()
        .filter(|t| t.kind == "reference")
        .map(|t| t.reference)
        .filter(|r| requested.is_empty() || requested.contains(r))
        .filter(|r| prefixes.is_empty() || prefixes.iter().any(|p| r.starts_with(p)))
        .collect();
    let mut updated = Vec::new();
    for reference in refs {
        let ref_ipc = reference.clone();
        let result = with_ipc(ctx.ipc.clone(), get_path(args, "board")?, move |c| {
            c.edit_reference_text(
                &ref_ipc,
                None,
                None,
                Some(width),
                Some(height),
                Some(stroke),
                None,
                None,
            )
        })
        .await?;
        match result {
            Ok(()) => updated.push(reference),
            Err(e) => {
                return Ok(CallToolResult::error(format!(
                    "IPC error while updating {}: {}",
                    reference, e
                )))
            }
        }
    }
    Ok(CallToolResult::json(
        &json!({ "updated_count": updated.len(), "references": updated }),
    ))
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn from_center(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            x0: x - w / 2.0,
            y0: y - h / 2.0,
            x1: x + w / 2.0,
            y1: y + h / 2.0,
        }
    }
    fn expand(self, d: f64) -> Self {
        Self {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }
    fn intersects(self, other: Self) -> bool {
        self.x0 < other.x1 && self.x1 > other.x0 && self.y0 < other.y1 && self.y1 > other.y0
    }
    fn contains(self, other: Self, margin: f64) -> bool {
        other.x0 >= self.x0 + margin
            && other.x1 <= self.x1 - margin
            && other.y0 >= self.y0 + margin
            && other.y1 <= self.y1 - margin
    }
}

#[derive(Default)]
struct BoardGeometry {
    outline: Option<Rect>,
    copper: Vec<(String, Rect, Option<Side>)>,
    pads_by_ref: HashMap<String, Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Front,
    Back,
}

fn layer_side(layer: &str) -> Option<Side> {
    if layer.starts_with("F.") {
        Some(Side::Front)
    } else if layer.starts_with("B.") {
        Some(Side::Back)
    } else {
        None
    }
}

fn node_xy(node: &SexpNode, tag: &str) -> Option<(f64, f64)> {
    let n = node.find(tag)?;
    Some((n.get_f64(1)?, n.get_f64(2)?))
}

fn rotated_point(x: f64, y: f64, ox: f64, oy: f64, degrees: f64) -> (f64, f64) {
    let a = degrees.to_radians();
    (
        ox + x * a.cos() - y * a.sin(),
        oy + x * a.sin() + y * a.cos(),
    )
}

fn union_rect(a: Option<Rect>, b: Rect) -> Rect {
    a.map_or(b, |a| Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    })
}

fn collect_edge_points(node: &SexpNode, points: &mut Vec<(f64, f64)>) {
    if node.find_str("layer") == Some("Edge.Cuts") {
        for tag in ["start", "end", "mid", "center"] {
            if let Some(p) = node_xy(node, tag) {
                points.push(p);
            }
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_edge_points(child, points);
        }
    }
}

fn parse_board_geometry(path: &std::path::Path) -> anyhow::Result<BoardGeometry> {
    let tree = parse_sexp(&std::fs::read_to_string(path)?)?;
    let mut geometry = BoardGeometry::default();
    let mut edge_points = Vec::new();
    collect_edge_points(&tree, &mut edge_points);
    if !edge_points.is_empty() {
        geometry.outline = Some(Rect {
            x0: edge_points
                .iter()
                .map(|p| p.0)
                .fold(f64::INFINITY, f64::min),
            y0: edge_points
                .iter()
                .map(|p| p.1)
                .fold(f64::INFINITY, f64::min),
            x1: edge_points
                .iter()
                .map(|p| p.0)
                .fold(f64::NEG_INFINITY, f64::max),
            y1: edge_points
                .iter()
                .map(|p| p.1)
                .fold(f64::NEG_INFINITY, f64::max),
        });
    }
    for fp in tree.find_all("footprint") {
        let at = fp.find("at");
        let (fx, fy, fr) = match at {
            Some(n) => (
                n.get_f64(1).unwrap_or(0.0),
                n.get_f64(2).unwrap_or(0.0),
                n.get_f64(3).unwrap_or(0.0),
            ),
            None => continue,
        };
        let reference = fp
            .find_all("property")
            .into_iter()
            .find(|p| p.get(1).and_then(SexpNode::as_str) == Some("Reference"))
            .and_then(|p| p.get(2))
            .and_then(SexpNode::as_str)
            .unwrap_or("?")
            .to_string();
        let mut own = None;
        for pad in fp.find_all("pad") {
            let (px, py) = node_xy(pad, "at").unwrap_or((0.0, 0.0));
            let pr = pad.find("at").and_then(|n| n.get_f64(3)).unwrap_or(0.0);
            let (cx, cy) = rotated_point(px, py, fx, fy, fr);
            let (mut w, mut h) = node_xy(pad, "size").unwrap_or((0.5, 0.5));
            let angle = (fr + pr).rem_euclid(180.0);
            if (angle - 90.0).abs() < 45.0 {
                std::mem::swap(&mut w, &mut h);
            }
            let rect = Rect::from_center(cx, cy, w, h);
            let layers = pad
                .find("layers")
                .and_then(SexpNode::children)
                .unwrap_or(&[]);
            let has_f = layers
                .iter()
                .filter_map(SexpNode::as_str)
                .any(|l| l == "F.Cu" || l == "*.Cu");
            let has_b = layers
                .iter()
                .filter_map(SexpNode::as_str)
                .any(|l| l == "B.Cu" || l == "*.Cu");
            let side = match (has_f, has_b) {
                (true, false) => Some(Side::Front),
                (false, true) => Some(Side::Back),
                _ => None,
            };
            geometry
                .copper
                .push((format!("pad {}", reference), rect, side));
            own = Some(union_rect(own, rect));
        }
        if let Some(rect) = own {
            geometry.pads_by_ref.insert(reference, rect);
        }
    }
    for via in tree.find_all("via") {
        if let Some((x, y)) = node_xy(via, "at") {
            let size = via.find_f64("size").unwrap_or(0.6);
            geometry
                .copper
                .push(("via".to_string(), Rect::from_center(x, y, size, size), None));
        }
    }
    Ok(geometry)
}

fn text_rect(
    text: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    stroke: f64,
    rotation: f64,
) -> Rect {
    let mut w = (text.chars().count() as f64 * width * 0.72 + stroke).max(width);
    let mut h = height + stroke;
    if (rotation.rem_euclid(180.0) - 90.0).abs() < 45.0 {
        std::mem::swap(&mut w, &mut h);
    }
    Rect::from_center(x, y, w, h)
}

fn selected_ref(reference: &str, requested: &[String], prefixes: &[String]) -> bool {
    (requested.is_empty() || requested.iter().any(|r| r == reference))
        && (prefixes.is_empty() || prefixes.iter().any(|p| reference.starts_with(p)))
}

fn geometry_conflicts(
    r: Rect,
    side: Option<Side>,
    geometry: &BoardGeometry,
    copper_clearance: f64,
    edge_clearance: f64,
) -> bool {
    geometry
        .outline
        .is_some_and(|o| !o.contains(r, edge_clearance))
        || geometry.copper.iter().any(|(_, c, copper_side)| {
            copper_side.is_none_or(|s| Some(s) == side) && r.intersects(c.expand(copper_clearance))
        })
}

async fn handle_check_reference_collisions(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let board = get_path(args, "board")?;
    let copper_clearance = optional_f64(args, "copper_clearance").unwrap_or(0.20);
    let edge_clearance = optional_f64(args, "edge_clearance").unwrap_or(0.30);
    let geometry = parse_board_geometry(&board)?;
    let texts = ipc!(ctx, args, |c| c.list_footprint_texts());
    let refs: Vec<_> = texts
        .into_iter()
        .filter(|t| t.kind == "reference" && t.visible)
        .collect();
    let rects: Vec<_> = refs
        .iter()
        .map(|t| {
            text_rect(
                &t.text,
                t.x,
                t.y,
                t.width,
                t.height,
                t.stroke_width,
                t.rotation,
            )
        })
        .collect();
    let mut collisions = Vec::new();
    for (i, t) in refs.iter().enumerate() {
        let r = rects[i];
        let mut reasons = Vec::new();
        if geometry
            .outline
            .is_some_and(|o| !o.contains(r, edge_clearance))
        {
            reasons.push("board edge".to_string());
        }
        let side = layer_side(&t.layer);
        for (name, c, copper_side) in &geometry.copper {
            if copper_side.is_none_or(|s| Some(s) == side)
                && r.intersects(c.expand(copper_clearance))
            {
                reasons.push(name.clone());
            }
        }
        for (j, other) in rects.iter().enumerate() {
            if i != j && layer_side(&refs[j].layer) == side && r.intersects(*other) {
                reasons.push(format!("reference {}", refs[j].reference));
            }
        }
        reasons.sort();
        reasons.dedup();
        if !reasons.is_empty() {
            collisions.push(json!({"reference": t.reference, "reasons": reasons}));
        }
    }
    Ok(CallToolResult::json(
        &json!({"visible_references": refs.len(), "collision_count": collisions.len(), "collisions": collisions}),
    ))
}

async fn handle_auto_place_references(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let board = get_path(args, "board")?;
    let width = optional_f64(args, "width").unwrap_or(1.0);
    let height = optional_f64(args, "height").unwrap_or(1.0);
    let stroke = optional_f64(args, "stroke_width").unwrap_or(0.15);
    if width < 1.0 || height < 1.0 || stroke < 0.15 {
        return Ok(CallToolResult::error(
            "JLCPCB minimum is 1.0 x 1.0 mm with 0.15 mm stroke",
        ));
    }
    let copper_clearance = optional_f64(args, "copper_clearance").unwrap_or(0.20);
    let edge_clearance = optional_f64(args, "edge_clearance").unwrap_or(0.30);
    let gap = optional_f64(args, "placement_gap").unwrap_or(0.35);
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let only_colliding = args
        .get("only_colliding")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let hide_passives = args
        .get("hide_unplaced_passives")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let requested: Vec<String> = args["references"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let prefixes: Vec<String> = args["prefixes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let geometry = parse_board_geometry(&board)?;
    let texts = ipc!(ctx, args, |c| c.list_footprint_texts());
    let all_refs: Vec<_> = texts
        .into_iter()
        .filter(|t| t.kind == "reference")
        .collect();
    let initial: Vec<_> = all_refs
        .iter()
        .map(|t| {
            (
                text_rect(
                    &t.text,
                    t.x,
                    t.y,
                    t.width,
                    t.height,
                    t.stroke_width,
                    t.rotation,
                ),
                layer_side(&t.layer),
            )
        })
        .collect();
    let mut colliding = vec![false; all_refs.len()];
    for i in 0..all_refs.len() {
        if all_refs[i].visible
            && geometry_conflicts(
                initial[i].0,
                initial[i].1,
                &geometry,
                copper_clearance,
                edge_clearance,
            )
        {
            colliding[i] = true;
        }
        if all_refs[i].visible {
            for j in 0..all_refs.len() {
                if i != j
                    && all_refs[j].visible
                    && initial[i].1 == initial[j].1
                    && initial[i].0.intersects(initial[j].0)
                {
                    colliding[i] = true;
                }
            }
        }
    }
    let mut refs: Vec<_> = all_refs
        .iter()
        .enumerate()
        .filter(|(_, t)| selected_ref(&t.reference, &requested, &prefixes))
        .map(|(i, t)| (i, (*t).clone()))
        .collect();
    refs.sort_by_key(|(_, t)| {
        if t.reference.starts_with(['J', 'U', 'Q', 'L', 'D']) {
            0
        } else {
            1
        }
    });
    let mut occupied: Vec<(Rect, Option<Side>)> = all_refs
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            t.visible
                && (!selected_ref(&t.reference, &requested, &prefixes)
                    || (only_colliding && !colliding[*i]))
        })
        .map(|(i, _)| initial[i])
        .collect();
    let mut actions = Vec::new();
    for (index, t) in refs {
        if only_colliding && t.visible && !colliding[index] {
            actions.push(json!({"reference": t.reference, "action": "keep"}));
            continue;
        }
        let Some(base) = geometry.pads_by_ref.get(&t.reference).copied() else {
            continue;
        };
        let side = layer_side(&t.layer);
        let tw = (t.reference.chars().count() as f64 * width * 0.72 + stroke).max(width);
        let th = height + stroke;
        let candidates = [
            (base.x0 + tw / 2.0, base.y0 - gap - th / 2.0, 0.0),
            (base.x0 + tw / 2.0, base.y1 + gap + th / 2.0, 0.0),
            (base.x0 - gap - th / 2.0, base.y0 + tw / 2.0, 90.0),
            (base.x1 + gap + th / 2.0, base.y0 + tw / 2.0, 90.0),
        ];
        let chosen = candidates.into_iter().find(|(x, y, rot)| {
            let r = text_rect(&t.reference, *x, *y, width, height, stroke, *rot);
            geometry
                .outline
                .is_none_or(|o| o.contains(r, edge_clearance))
                && !geometry.copper.iter().any(|(_, c, copper_side)| {
                    copper_side.is_none_or(|s| Some(s) == side)
                        && r.intersects(c.expand(copper_clearance))
                })
                && !occupied
                    .iter()
                    .any(|(o, other_side)| *other_side == side && r.intersects(*o))
        });
        if let Some((x, y, rotation)) = chosen {
            occupied.push((
                text_rect(&t.reference, x, y, width, height, stroke, rotation),
                side,
            ));
            actions.push(json!({"reference": t.reference, "action": "place", "x": x, "y": y, "rotation": rotation}));
            if !dry_run {
                let reference = t.reference.clone();
                match with_ipc(ctx.ipc.clone(), get_path(args, "board")?, move |c| {
                    c.edit_reference_text(
                        &reference,
                        Some(x),
                        Some(y),
                        Some(width),
                        Some(height),
                        Some(stroke),
                        Some(rotation),
                        Some(true),
                    )
                })
                .await?
                {
                    Ok(()) => {}
                    Err(e) => {
                        return Ok(CallToolResult::error(format!("IPC update failed: {}", e)))
                    }
                }
            }
        } else {
            let passive = t.reference.starts_with('R') || t.reference.starts_with('C');
            let hide = passive && hide_passives;
            actions.push(
                json!({"reference": t.reference, "action": if hide {"hide"} else {"unplaced"}}),
            );
            if hide && !dry_run {
                let reference = t.reference.clone();
                match with_ipc(ctx.ipc.clone(), get_path(args, "board")?, move |c| {
                    c.edit_reference_text(
                        &reference,
                        None,
                        None,
                        Some(width),
                        Some(height),
                        Some(stroke),
                        None,
                        Some(false),
                    )
                })
                .await?
                {
                    Ok(()) => {}
                    Err(e) => {
                        return Ok(CallToolResult::error(format!("IPC update failed: {}", e)))
                    }
                }
            }
        }
    }
    Ok(CallToolResult::json(
        &json!({"dry_run": dry_run, "action_count": actions.len(), "actions": actions}),
    ))
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn handle_place_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let footprint = match require_str(args, "footprint") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let x = match require_f64(args, "x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y = match require_f64(args, "y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let rotation = args["rotation"].as_f64().unwrap_or(0.0);
    let layer = args["layer"].as_str().unwrap_or("F.Cu").to_string();

    let fp = ipc!(ctx, args, |c| c
        .place_footprint(&footprint, x, y, rotation, &layer));
    Ok(CallToolResult::json(&json!({
        "placed": fp.reference,
        "footprint": fp.footprint,
        "x": fp.position.x, "y": fp.position.y,
        "definition_anchor": { "x": fp.definition_anchor.x, "y": fp.definition_anchor.y },
        "definition_item_samples": fp.definition_item_samples,
        "rotation": fp.rotation, "layer": fp.layer
    })))
}

async fn handle_replace_component_footprint(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let footprint = match require_str(args, "footprint") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let (library_nickname, entry_name) = match footprint.split_once(':') {
        Some(parts) => parts,
        None => {
            return Ok(CallToolResult::error(
                "footprint must use the Library:Footprint form",
            ))
        }
    };
    let kicad_root = std::path::Path::new(&ctx.config.kicad_cli)
        .parent()
        .and_then(std::path::Path::parent);
    let Some(kicad_root) = kicad_root else {
        return Ok(CallToolResult::error(
            "Cannot derive the KiCad installation root from kicad_cli",
        ));
    };
    let footprint_path = kicad_root
        .join("share")
        .join("kicad")
        .join("footprints")
        .join(format!("{}.pretty", library_nickname))
        .join(format!("{}.kicad_mod", entry_name));
    let library_contents = match tokio::fs::read_to_string(&footprint_path).await {
        Ok(contents) => contents,
        Err(error) => {
            return Ok(CallToolResult::error(format!(
                "Cannot read library footprint '{}': {}",
                footprint_path.display(),
                error
            )))
        }
    };
    let value = match require_str(args, "value") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let x = match require_f64(args, "x") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let y = match require_f64(args, "y") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let rotation = args["rotation"].as_f64().unwrap_or(0.0);
    let layer = args["layer"].as_str().unwrap_or("F.Cu").to_string();
    let exclude_from_bom = args["exclude_from_bom"].as_bool().unwrap_or(false);
    let dnp = args["dnp"].as_bool().unwrap_or(false);
    let pad_nets: Vec<(String, String)> = match args["pad_nets"].as_object() {
        Some(mapping) => {
            let mut values = Vec::with_capacity(mapping.len());
            for (pad, net) in mapping {
                let Some(net) = net.as_str() else {
                    return Ok(CallToolResult::error(format!(
                        "pad_nets['{}'] must be a string",
                        pad
                    )));
                };
                values.push((pad.clone(), net.to_string()));
            }
            values
        }
        None => return Ok(CallToolResult::error("pad_nets must be an object")),
    };

    let reference_ipc = reference.clone();
    let footprint_ipc = footprint.clone();
    let library_contents_ipc = library_contents.clone();
    let value_ipc = value.clone();
    let result = ipc!(ctx, args, |client| client.replace_footprint_from_library(
        &reference_ipc,
        &footprint_ipc,
        &library_contents_ipc,
        &value_ipc,
        x,
        y,
        rotation,
        &layer,
        &pad_nets,
        exclude_from_bom,
        dnp,
    ));
    Ok(CallToolResult::json(&json!({
        "reference": result.reference,
        "value": result.value,
        "footprint": result.footprint,
        "x": result.position.x,
        "y": result.position.y,
        "rotation": result.rotation,
        "layer": result.layer,
        "exclude_from_bom": result.exclude_from_bom,
        "dnp": result.dnp
    })))
}

async fn handle_move_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let x = match require_f64(args, "x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y = match require_f64(args, "y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    let lookup_reference = reference.clone();
    let before = ipc!(ctx, args, |c| c.get_footprint(&lookup_reference))
        .ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", reference))?;
    let move_reference = reference.clone();
    let rotation = args.get("rotation").and_then(|value| value.as_f64());
    if let Some(rotation) = rotation {
        ipc!(ctx, args, |client| client.transform_footprint(
            &move_reference,
            x,
            y,
            rotation
        ));
    } else {
        ipc!(ctx, args, |client| client.move_footprint(
            &move_reference,
            x,
            y
        ));
    }
    Ok(CallToolResult::json(&json!({
        "moved": reference,
        "from": { "x": before.position.x, "y": before.position.y, "rotation": before.rotation },
        "x": x,
        "y": y,
        "rotation": rotation.unwrap_or(before.rotation),
        "method": "kicad_ipc_commit"
    })))
}

async fn handle_rotate_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let rotation = match require_f64(args, "rotation") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    let reference_query = reference.clone();
    let before = ipc!(ctx, args, |c| c.get_footprint(&reference_query));
    let reference_ipc = reference.clone();
    ipc!(ctx, args, |c| c.rotate_footprint(&reference_ipc, rotation));
    Ok(CallToolResult::json(&json!({
        "rotated": reference,
        "from": before.as_ref().map(|footprint| footprint.rotation),
        "rotation": rotation,
        "method": "kicad_ipc_commit"
    })))
}

async fn handle_set_component_pad_relative_angle(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let relative_angle = match require_f64(args, "relative_angle") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };

    let repair_reference = reference.clone();
    let changed = ipc!(ctx, args, |client| client
        .set_footprint_pad_relative_angle(&repair_reference, relative_angle));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "relative_angle": relative_angle,
        "pads": changed,
        "method": "kicad_ipc_commit"
    })))
}

async fn handle_flip_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };

    let lookup_reference = reference.clone();
    let before = ipc!(ctx, args, |client| client.get_footprint(&lookup_reference))
        .ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", reference))?;
    let flip_reference = reference.clone();
    ipc!(ctx, args, |client| client.flip_footprint(&flip_reference));
    let verify_reference = reference.clone();
    let after = ipc!(ctx, args, |client| client.get_footprint(&verify_reference))
        .ok_or_else(|| anyhow::anyhow!("Footprint '{}' missing after flip", reference))?;

    if before.layer == after.layer {
        return Ok(CallToolResult::error(format!(
            "KiCad flip action did not change the layer for '{}'",
            reference
        )));
    }

    Ok(CallToolResult::json(&json!({
        "flipped": reference,
        "from_layer": before.layer,
        "layer": after.layer,
        "x": after.position.x,
        "y": after.position.y,
        "rotation": after.rotation,
        "method": "kicad_ipc_native_action"
    })))
}

fn update_footprint_transform(
    content: &str,
    reference: &str,
    new_x: Option<f64>,
    new_y: Option<f64>,
    new_rotation: Option<f64>,
) -> Option<(String, f64, f64, f64)> {
    let needle = format!("(property \"Reference\" \"{}\"", reference);
    let reference_pos = content.find(&needle)?;
    let footprint_start = content[..reference_pos].rfind("(footprint ")?;
    let (_, footprint_end) = find_block_with_leading_whitespace(content, footprint_start)?;
    if reference_pos >= footprint_end {
        return None;
    }

    let block = &content[footprint_start..footprint_end];
    let at_rel = block.find("(at ")?;
    let at_start = footprint_start + at_rel;
    let at_end = at_start + content[at_start..].find(')')? + 1;
    if at_end > footprint_end {
        return None;
    }

    let fields: Vec<&str> = content[at_start + 4..at_end - 1]
        .split_whitespace()
        .collect();
    if fields.len() < 2 {
        return None;
    }
    let old_x = fields[0].parse::<f64>().ok()?;
    let old_y = fields[1].parse::<f64>().ok()?;
    let old_rotation = fields
        .get(2)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let x = new_x.unwrap_or(old_x);
    let y = new_y.unwrap_or(old_y);
    let rotation = new_rotation.unwrap_or(old_rotation);
    let replacement = if rotation.abs() < f64::EPSILON {
        format!("(at {x} {y})")
    } else {
        format!("(at {x} {y} {rotation})")
    };

    let new_content = format!(
        "{}{}{}",
        &content[..at_start],
        replacement,
        &content[at_end..]
    );
    Some((new_content, old_x, old_y, old_rotation))
}

async fn handle_delete_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };

    let ref_ipc = reference.clone();
    ipc!(ctx, args, |c| c.delete_footprint(&ref_ipc));
    Ok(CallToolResult::json(&json!({ "deleted": reference })))
}

async fn handle_edit_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let before_reference = reference.clone();
    let before = ipc!(ctx, args, |client| client
        .get_footprint(&before_reference)?
        .ok_or_else(|| anyhow::anyhow!(
            "Footprint '{}' not found",
            before_reference
        )));
    let requested_exclude = args
        .get("exclude_from_bom")
        .and_then(|value| value.as_bool());
    let requested_dnp = args.get("dnp").and_then(|value| value.as_bool());
    let width = optional_f64(args, "width");
    let height = optional_f64(args, "height");
    let stroke = optional_f64(args, "stroke_width");
    let x = optional_f64(args, "x");
    let y = optional_f64(args, "y");
    let rotation = optional_f64(args, "rotation");
    let visible = args.get("visible").and_then(|value| value.as_bool());
    if width.is_some_and(|value| value < 1.0) || height.is_some_and(|value| value < 1.0) {
        return Ok(CallToolResult::error(
            "JLCPCB text width/height must be >= 1.0 mm",
        ));
    }
    if stroke.is_some_and(|value| value < 0.15) {
        return Ok(CallToolResult::error(
            "JLCPCB silkscreen stroke width must be >= 0.15 mm",
        ));
    }
    let exclude_from_bom = requested_exclude.unwrap_or(before.exclude_from_bom);
    let dnp = requested_dnp.unwrap_or(before.dnp);
    if requested_exclude.is_some() || requested_dnp.is_some() {
        let update_reference = reference.clone();
        ipc!(ctx, args, |client| client
            .set_footprint_assembly_attributes(
                &update_reference,
                exclude_from_bom,
                dnp
            ));
    }
    let edits_reference_text = [x, y, width, height, stroke, rotation]
        .into_iter()
        .any(|value| value.is_some())
        || visible.is_some();
    if edits_reference_text {
        let text_reference = reference.clone();
        ipc!(ctx, args, |client| client.edit_reference_text(
            &text_reference,
            x,
            y,
            width,
            height,
            stroke,
            rotation,
            visible
        ));
    }
    let after_reference = reference.clone();
    let after = ipc!(ctx, args, |client| client
        .get_footprint(&after_reference)?
        .ok_or_else(|| anyhow::anyhow!(
            "Footprint '{}' missing after update",
            after_reference
        )));
    if after.exclude_from_bom != exclude_from_bom || after.dnp != dnp {
        return Ok(CallToolResult::error(format!(
            "KiCad did not retain requested assembly attributes for '{}'",
            reference
        )));
    }

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "before": {
            "exclude_from_bom": before.exclude_from_bom,
            "dnp": before.dnp
        },
        "after": {
            "exclude_from_bom": after.exclude_from_bom,
            "dnp": after.dnp
        },
        "value": after.value,
        "footprint": after.footprint,
        "method": if requested_exclude.is_some() || requested_dnp.is_some() {
            "kicad_ipc_commit"
        } else if edits_reference_text {
            "kicad_ipc_reference_text_commit"
        } else {
            "query_only"
        },
        "note": if args.get("value").is_some() {
            "Value edits via IPC are not yet supported; edit the schematic and update PCB."
        } else {
            ""
        }
    })))
}

async fn handle_find_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let fp = ipc!(ctx, args, |c| {
        c.get_footprint(&reference)?
            .ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", reference))
    });
    Ok(CallToolResult::json(&json!({
        "reference": fp.reference,
        "value": fp.value,
        "footprint": fp.footprint,
        "x": fp.position.x, "y": fp.position.y,
        "definition_anchor": { "x": fp.definition_anchor.x, "y": fp.definition_anchor.y },
        "definition_item_samples": fp.definition_item_samples,
        "definition_item_types": fp.definition_item_types,
        "exclude_from_bom": fp.exclude_from_bom,
        "dnp": fp.dnp,
        "rotation": fp.rotation, "layer": fp.layer
    })))
}

async fn handle_get_component_pads(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };

    let reference_ipc = reference.clone();
    let live_pads = ipc!(ctx, args, |c| c.get_footprint_pads(&reference_ipc));
    let pads: Vec<serde_json::Value> = live_pads
        .into_iter()
        .map(|pad| {
            json!({
                "number": pad.number,
                "x": pad.position.x,
                "y": pad.position.y,
                "net": pad.net
            })
        })
        .collect();

    Ok(CallToolResult::json(
        &json!({ "reference": reference, "pad_count": pads.len(), "pads": pads }),
    ))
}

async fn handle_set_component_pad_nets(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let Some(mapping) = args["pad_nets"].as_object() else {
        return Ok(CallToolResult::error(
            "pad_nets must be an object mapping pad numbers to net names",
        ));
    };
    if mapping.is_empty() {
        return Ok(CallToolResult::error("pad_nets must not be empty"));
    }
    let mut pad_nets = Vec::with_capacity(mapping.len());
    for (pad_number, net_name) in mapping {
        let parsed_net_name = if net_name.is_null() {
            None
        } else if let Some(net_name) = net_name.as_str() {
            Some(net_name.to_string())
        } else {
            return Ok(CallToolResult::error(format!(
                "Net assignment for pad '{}' must be a string or null",
                pad_number
            )));
        };
        pad_nets.push((pad_number.clone(), parsed_net_name));
    }

    let reference_ipc = reference.clone();
    let pad_nets_ipc = pad_nets.clone();
    ipc!(ctx, args, |client| client
        .set_footprint_pad_nets(&reference_ipc, &pad_nets_ipc));

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "pad_nets": mapping,
        "source": "ipc"
    })))
}

async fn handle_replace_component_pad_layout(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = require_str(args, "reference")
        .map_err(|e| anyhow::anyhow!("{:?}", e))?
        .to_string();
    let pads = args["pads"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("pads must be an array"))?
        .clone();
    let pad_count = pads.len();
    let description = args["description"].as_str().map(str::to_string);
    let reference_ipc = reference.clone();
    let result = ipc!(ctx, args, |client| {
        client.replace_footprint_pad_layout(&reference_ipc, &pads, description.as_deref())
    });
    Ok(CallToolResult::text(
        serde_json::to_string_pretty(&json!({
            "success": true,
            "reference": reference,
            "pad_count": pad_count,
            "result": result
        }))
        .unwrap(),
    ))
}

async fn handle_normalize_two_pad_smd_footprint(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = require_str(args, "reference")
        .map_err(|e| anyhow::anyhow!("{:?}", e))?
        .to_string();
    let library_nickname = require_str(args, "library_nickname")
        .map_err(|e| anyhow::anyhow!("{:?}", e))?
        .to_string();
    let entry_name = require_str(args, "entry_name")
        .map_err(|e| anyhow::anyhow!("{:?}", e))?
        .to_string();
    let description = args["description"].as_str().map(str::to_string);
    let keywords = args["keywords"].as_str().map(str::to_string);
    let value = args["value"].as_str().map(str::to_string);
    let spacing = args["pad_spacing"].as_f64();
    let width = args["pad_width"].as_f64();
    let height = args["pad_height"].as_f64();
    let pad_roundrect_ratio = args["pad_roundrect_ratio"].as_f64();
    let courtyard_half_span = args["courtyard_half_span"].as_f64();
    let silk_segment_half_length = args["silk_segment_half_length"].as_f64();
    let reference_ipc = reference.clone();
    let library_nickname_ipc = library_nickname.clone();
    let entry_name_ipc = entry_name.clone();
    let description_ipc = description.clone();
    let keywords_ipc = keywords.clone();
    let value_ipc = value.clone();
    let changed_pads = ipc!(ctx, args, |client| client.normalize_two_pad_smd_footprint(
        &reference_ipc,
        &library_nickname_ipc,
        &entry_name_ipc,
        value_ipc.as_deref(),
        description_ipc.as_deref(),
        keywords_ipc.as_deref(),
        spacing,
        width,
        height,
        pad_roundrect_ratio,
        courtyard_half_span,
        silk_segment_half_length,
    ));
    Ok(CallToolResult::json(&json!({
        "success": true,
        "reference": reference,
        "footprint": format!("{}:{}", library_nickname, entry_name),
        "changed_pads": changed_pads,
        "source": "ipc"
    })))
}

async fn handle_set_component_3d_model(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let filename = match require_str(args, "filename") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let offset = [
        args["offset_x"].as_f64().unwrap_or(0.0),
        args["offset_y"].as_f64().unwrap_or(0.0),
        args["offset_z"].as_f64().unwrap_or(0.0),
    ];
    let rotation = [
        args["rotation_x"].as_f64().unwrap_or(0.0),
        args["rotation_y"].as_f64().unwrap_or(0.0),
        args["rotation_z"].as_f64().unwrap_or(0.0),
    ];
    let scale = [
        args["scale_x"].as_f64().unwrap_or(1.0),
        args["scale_y"].as_f64().unwrap_or(1.0),
        args["scale_z"].as_f64().unwrap_or(1.0),
    ];
    let reference_ipc = reference.clone();
    let filename_ipc = filename.clone();
    ipc!(ctx, args, |client| client.set_footprint_3d_model(
        &reference_ipc,
        &filename_ipc,
        offset,
        rotation,
        scale,
    ));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "filename": filename,
        "offset_mm": offset,
        "rotation": rotation,
        "scale": scale,
        "source": "ipc"
    })))
}

async fn handle_update_footprint_mechanical_geometry(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let zone_index = args["zone_index"].as_u64().unwrap_or(0) as usize;
    let zone_values = args.get("zone_points").and_then(|value| value.as_array());
    let mut zone_points = Vec::with_capacity(zone_values.map_or(0, Vec::len));
    if let Some(zone_values) = zone_values {
        if zone_values.len() < 3 {
            return Ok(CallToolResult::error(
                "zone_points must contain at least 3 points when supplied",
            ));
        }
        for point in zone_values {
            let Some(x) = point["x"].as_f64() else {
                return Ok(CallToolResult::error("Every zone point requires numeric x"));
            };
            let Some(y) = point["y"].as_f64() else {
                return Ok(CallToolResult::error("Every zone point requires numeric y"));
            };
            zone_points.push((x, y));
        }
    }
    let zone_layers = args["zone_layers"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| anyhow::anyhow!("Every zone layer must be a string"))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let courtyard_layer = args["courtyard_layer"]
        .as_str()
        .unwrap_or("F.CrtYd")
        .to_string();
    let courtyard_index = args["courtyard_index"].as_u64().unwrap_or(0) as usize;
    let courtyard_x1 = match require_f64(args, "courtyard_x1") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let courtyard_y1 = match require_f64(args, "courtyard_y1") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let courtyard_x2 = match require_f64(args, "courtyard_x2") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let courtyard_y2 = match require_f64(args, "courtyard_y2") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };

    let reference_ipc = reference.clone();
    let zone_points_ipc = zone_points.clone();
    let zone_layers_ipc = zone_layers.clone();
    let courtyard_layer_ipc = courtyard_layer.clone();
    ipc!(ctx, args, |client| client
        .update_footprint_mechanical_geometry(
            &reference_ipc,
            zone_index,
            &zone_points_ipc,
            &zone_layers_ipc,
            &courtyard_layer_ipc,
            courtyard_index,
            courtyard_x1,
            courtyard_y1,
            courtyard_x2,
            courtyard_y2,
        ));

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "zone_index": zone_values.map(|_| json!(zone_index)).unwrap_or(serde_json::Value::Null),
        "zone_points": zone_values.map_or(serde_json::Value::Null, |values| json!(values)),
        "zone_coordinate_space": "board_absolute",
        "zone_layers": if zone_layers.is_empty() { serde_json::Value::Null } else { json!(zone_layers) },
        "courtyard_layer": courtyard_layer,
        "courtyard_index": courtyard_index,
        "courtyard_coordinate_space": "board_absolute",
        "courtyard": {
            "x1": courtyard_x1,
            "y1": courtyard_y1,
            "x2": courtyard_x2,
            "y2": courtyard_y2
        },
        "source": "ipc"
    })))
}

async fn handle_replace_footprint_graphic_segments(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let layers: Vec<String> = match args.get("layers").and_then(|value| value.as_array()) {
        Some(values) if !values.is_empty() => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => {
            return Ok(CallToolResult::error(
                "'layers' must be a non-empty string array",
            ))
        }
    };
    if layers.len()
        != args
            .get("layers")
            .and_then(|value| value.as_array())
            .map_or(0, Vec::len)
    {
        return Ok(CallToolResult::error(
            "every 'layers' entry must be a string",
        ));
    }
    let segments = match args.get("segments").and_then(|value| value.as_array()) {
        Some(values) => values.clone(),
        None => return Ok(CallToolResult::error("'segments' must be an array")),
    };

    let reference_ipc = reference.clone();
    let layers_ipc = layers.clone();
    let (removed, added) = ipc!(ctx, args, |client| client
        .replace_footprint_graphic_segments(&reference_ipc, &layers_ipc, &segments));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "layers": layers,
        "removed": removed,
        "added": added
    })))
}

async fn handle_delete_footprint_nested_zones(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let reference_ipc = reference.clone();
    let removed = ipc!(ctx, args, |client| client
        .delete_footprint_nested_zones(&reference_ipc));

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "removed_zone_count": removed,
        "source": "ipc"
    })))
}

async fn handle_set_footprint_graphics_layer(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let from_layer = match require_str(args, "from_layer") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let to_layer = match require_str(args, "to_layer") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let mut text_updates = Vec::new();
    if let Some(values) = args.get("text_updates").and_then(|value| value.as_array()) {
        for value in values {
            let Some(match_text) = value.get("match_text").and_then(|item| item.as_str()) else {
                return Ok(CallToolResult::error(
                    "Every text_updates item requires match_text",
                ));
            };
            text_updates.push(konnect_ipc::IpcFootprintUserTextUpdate {
                match_text: match_text.to_string(),
                new_text: value
                    .get("new_text")
                    .and_then(|item| item.as_str())
                    .map(str::to_string),
                x: value.get("x").and_then(|item| item.as_f64()),
                y: value.get("y").and_then(|item| item.as_f64()),
                rotation: value.get("rotation").and_then(|item| item.as_f64()),
                layer: value
                    .get("layer")
                    .and_then(|item| item.as_str())
                    .map(str::to_string),
            });
        }
    }
    let reference_ipc = reference.clone();
    let from_layer_ipc = from_layer.clone();
    let to_layer_ipc = to_layer.clone();
    let text_updates_ipc = text_updates.clone();
    let (shapes_changed, texts_changed, texts_updated) = ipc!(ctx, args, |client| client
        .set_footprint_graphics_layer(
            &reference_ipc,
            &from_layer_ipc,
            &to_layer_ipc,
            &text_updates_ipc,
        ));

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "from_layer": from_layer,
        "to_layer": to_layer,
        "shapes_changed": shapes_changed,
        "texts_changed": texts_changed,
        "texts_updated": texts_updated,
        "source": "ipc"
    })))
}

async fn handle_replace_footprint_user_texts(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let specs = args["texts"].as_array().cloned().unwrap_or_default();
    let reference_ipc = reference.clone();
    let (removed, added) = ipc!(ctx, args, |client| client
        .replace_footprint_user_texts(&reference_ipc, &specs));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "removed": removed,
        "added": added,
        "source": "ipc"
    })))
}

async fn handle_get_component_3d_models(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let reference_ipc = reference.clone();
    let models = ipc!(ctx, args, |client| client
        .get_footprint_3d_models(&reference_ipc));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "model_count": models.len(),
        "models": models,
        "source": "ipc"
    })))
}

async fn handle_set_component_3d_model_transform(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let _board_path = get_path(args, "board")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let model_index = args["model_index"].as_u64().unwrap_or(0) as usize;
    let offset_mm = [
        args["offset_x"].as_f64().unwrap_or(0.0),
        args["offset_y"].as_f64().unwrap_or(0.0),
        args["offset_z"].as_f64().unwrap_or(0.0),
    ];
    let rotation = [
        args["rotation_x"].as_f64().unwrap_or(0.0),
        args["rotation_y"].as_f64().unwrap_or(0.0),
        args["rotation_z"].as_f64().unwrap_or(0.0),
    ];
    let reference_ipc = reference.clone();
    ipc!(ctx, args, |client| client.set_footprint_3d_model_transform(
        &reference_ipc,
        model_index,
        offset_mm,
        rotation
    ));
    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "model_index": model_index,
        "offset_mm": offset_mm,
        "rotation": rotation,
        "source": "ipc"
    })))
}

async fn handle_get_pad_position(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let pad_number = match require_str(args, "pad_number") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let pads_result = handle_get_component_pads(args, ctx).await?;
    // Parse the result and filter for the specific pad number
    if let Some(crate::mcp::protocol::ToolContent::Text { text }) = pads_result.content.first() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(pads) = parsed["pads"].as_array() {
                if let Some(pad) = pads
                    .iter()
                    .find(|p| p["number"].as_str() == Some(&pad_number))
                {
                    return Ok(CallToolResult::json(pad));
                }
            }
        }
    }
    Ok(CallToolResult::error(format!(
        "Pad '{}' not found",
        pad_number
    )))
}

async fn handle_get_component_list(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let fps = ipc!(ctx, args, |c| c.list_footprints());
    let items: Vec<serde_json::Value> = fps
        .iter()
        .map(|fp| {
            json!({
                "reference": fp.reference,
                "value": fp.value,
                "footprint": fp.footprint,
                "x": fp.position.x, "y": fp.position.y,
                "rotation": fp.rotation, "layer": fp.layer
                ,"exclude_from_bom": fp.exclude_from_bom
                ,"dnp": fp.dnp
            })
        })
        .collect();
    Ok(CallToolResult::json(
        &json!({ "count": items.len(), "components": items }),
    ))
}

async fn handle_place_array(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let footprint = match require_str(args, "footprint") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let start_x = match require_f64(args, "start_x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let start_y = match require_f64(args, "start_y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let count_x = args["count_x"].as_u64().unwrap_or(1) as usize;
    let count_y = args["count_y"].as_u64().unwrap_or(1) as usize;
    let spacing_x = match require_f64(args, "spacing_x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let spacing_y = args["spacing_y"].as_f64().unwrap_or(spacing_x);
    let prefix = args["ref_prefix"].as_str().unwrap_or("U").to_string();
    let ref_start = args["ref_start"].as_u64().unwrap_or(1) as usize;

    let mut placed = Vec::new();
    let mut n = ref_start;
    for row in 0..count_y {
        for col in 0..count_x {
            let x = start_x + col as f64 * spacing_x;
            let y = start_y + row as f64 * spacing_y;
            let reference = format!("{prefix}{n}");
            let fp_id = footprint.clone();
            let ref2 = reference.clone();
            match with_ipc(ctx.ipc.clone(), get_path(args, "board")?, move |c| {
                c.place_footprint(&fp_id, x, y, 0.0, "F.Cu")
            })
            .await?
            {
                Ok(fp) => placed
                    .push(json!({ "reference": ref2, "x": fp.position.x, "y": fp.position.y })),
                Err(e) => {
                    return Ok(CallToolResult::error(format!(
                        "IPC error placing {}: {}",
                        reference, e
                    )))
                }
            }
            n += 1;
        }
    }
    Ok(CallToolResult::json(
        &json!({ "placed_count": placed.len(), "components": placed }),
    ))
}

async fn handle_align_components(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let refs = args["references"].as_array().cloned().unwrap_or_default();
    let axis = args["axis"].as_str().unwrap_or("x").to_string();
    let value = match require_f64(args, "value") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    let mut aligned = Vec::new();
    for ref_val in &refs {
        let reference = match ref_val.as_str() {
            Some(r) => r.to_string(),
            None => continue,
        };
        let ref2 = reference.clone();
        let axis_clone = axis.clone();
        let res = with_ipc(ctx.ipc.clone(), get_path(args, "board")?, move |c| {
            let fp = c
                .get_footprint(&ref2)?
                .ok_or_else(|| anyhow::anyhow!("not found"))?;
            let (nx, ny) = if axis_clone == "y" {
                (fp.position.x, value)
            } else {
                (value, fp.position.y)
            };
            c.move_footprint(&ref2, nx, ny)?;
            Ok((nx, ny))
        })
        .await?;
        match res {
            Ok((nx, ny)) => aligned.push(json!({ "reference": reference, "x": nx, "y": ny })),
            Err(e) => return Ok(CallToolResult::error(format!("IPC error: {}", e))),
        }
    }
    Ok(CallToolResult::json(
        &json!({ "aligned_count": aligned.len(), "components": aligned }),
    ))
}

async fn handle_duplicate_component(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let _new_reference = match require_str(args, "new_reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let x = match require_f64(args, "x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y = match require_f64(args, "y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    // Get the source footprint's footprint ID and rotation
    let ref_ipc = reference.clone();
    let src = ipc!(ctx, args, |c| {
        c.get_footprint(&ref_ipc)?
            .ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", ref_ipc))
    });

    let fp = ipc!(ctx, args, |c| c.place_footprint(
        &src.footprint,
        x,
        y,
        src.rotation,
        &src.layer
    ));
    Ok(CallToolResult::json(&json!({
        "duplicated_from": reference,
        "new_reference": fp.reference,
        "x": fp.position.x, "y": fp.position.y
    })))
}

async fn handle_clone_component_instance(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let source_reference = match require_str(args, "source_reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let new_reference = match require_str(args, "new_reference") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let value = match require_str(args, "value") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let footprint = match require_str(args, "footprint") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let x = match require_f64(args, "x") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let y = match require_f64(args, "y") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let rotation = args["rotation"].as_f64().unwrap_or(0.0);
    let layer = args["layer"].as_str().unwrap_or("F.Cu").to_string();
    let symbol_path = match require_str(args, "symbol_path") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let sheet_name = match require_str(args, "sheet_name") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let sheet_file = match require_str(args, "sheet_file") {
        Ok(value) => value.to_string(),
        Err(error) => return Ok(error),
    };
    let model_filename = args["model_filename"].as_str().map(str::to_string);
    let exclude_from_bom = args["exclude_from_bom"].as_bool().unwrap_or(false);
    let dnp = args["dnp"].as_bool().unwrap_or(false);
    let source_ipc = source_reference.clone();
    let new_ipc = new_reference.clone();
    let result = ipc!(ctx, args, |client| client.clone_footprint_instance(
        &source_ipc,
        &new_ipc,
        &value,
        &footprint,
        x,
        y,
        rotation,
        &layer,
        &symbol_path,
        &sheet_name,
        &sheet_file,
        model_filename.as_deref(),
        exclude_from_bom,
        dnp
    ));
    Ok(CallToolResult::json(&json!({
        "reference": result.reference,
        "footprint": result.footprint,
        "x": result.position.x,
        "y": result.position.y,
        "rotation": result.rotation,
        "layer": result.layer
    })))
}

async fn handle_get_board_2d_view(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    use base64::Engine;
    let board_path = get_path(args, "board")?;
    let layers: Vec<String> = args["layers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "F.Cu".into(),
                "B.Cu".into(),
                "F.SilkS".into(),
                "B.SilkS".into(),
                "Edge.Cuts".into(),
            ]
        });

    let temp_dir = tempfile::tempdir()?;
    let tmp = temp_dir.path().join("board-render.png");
    let layer_refs: Vec<&str> = layers.iter().map(String::as_str).collect();
    super::cli::render_pcb_png(&ctx.config.kicad_cli, &board_path, &tmp, &layer_refs).await?;
    let bytes = tokio::fs::read(&tmp).await?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(CallToolResult::image(b64, "image/png"))
}

#[cfg(test)]
mod reference_layout_tests {
    use super::*;

    #[test]
    fn text_bounds_follow_rotation() {
        let horizontal = text_rect("R20", 10.0, 20.0, 1.0, 1.0, 0.15, 0.0);
        let vertical = text_rect("R20", 10.0, 20.0, 1.0, 1.0, 0.15, 90.0);
        assert!(horizontal.x1 - horizontal.x0 > horizontal.y1 - horizontal.y0);
        assert!(vertical.y1 - vertical.y0 > vertical.x1 - vertical.x0);
    }

    #[test]
    fn side_matching_distinguishes_front_and_back() {
        assert_eq!(layer_side("F.SilkS"), Some(Side::Front));
        assert_eq!(layer_side("B.Cu"), Some(Side::Back));
        assert_eq!(layer_side("Edge.Cuts"), None);
    }

    #[test]
    fn rectangle_clearance_is_conservative() {
        let text = Rect::from_center(0.0, 0.0, 1.0, 1.0);
        let pad = Rect::from_center(0.8, 0.0, 0.2, 0.2).expand(0.3);
        assert!(text.intersects(pad));
    }

    #[test]
    fn courtyard_overlap_requires_positive_area() {
        use konnect_ipc::types::{IpcBounds, IpcVector2};
        let a = IpcBounds {
            min: IpcVector2 { x: 0.0, y: 0.0 },
            max: IpcVector2 { x: 2.0, y: 2.0 },
        };
        let overlap = IpcBounds {
            min: IpcVector2 { x: 1.0, y: 1.0 },
            max: IpcVector2 { x: 3.0, y: 3.0 },
        };
        let touching = IpcBounds {
            min: IpcVector2 { x: 2.0, y: 0.0 },
            max: IpcVector2 { x: 3.0, y: 1.0 },
        };
        assert!(bounds_overlap(&a, &overlap, 0.0));
        assert!(!bounds_overlap(&a, &touching, 0.0));
        assert!(bounds_overlap(&a, &touching, 0.01));
    }

    #[test]
    fn file_move_changes_only_top_level_footprint_transform() {
        let board = r#"(kicad_pcb
  (footprint "Test:Part"
    (layer "F.Cu")
    (at 25.525 65.915)
    (property "Reference" "J1" (at 0 -5.5 0))
    (fp_line (start -4.58 -1.85) (end 4.58 -1.85))
    (pad "1" smd rect (at -1 0) (size 1 1))
  )
)"#;
        let (moved, old_x, old_y, old_rotation) =
            update_footprint_transform(board, "J1", Some(20.0), Some(4.85), None).unwrap();
        assert_eq!((old_x, old_y, old_rotation), (25.525, 65.915, 0.0));
        assert!(moved.contains("(at 20 4.85)"));
        assert!(moved.contains("(fp_line (start -4.58 -1.85)"));
        assert!(moved.contains("(pad \"1\" smd rect (at -1 0)"));
        assert!(moved.contains("(property \"Reference\" \"J1\" (at 0 -5.5 0))"));
    }

    #[test]
    fn file_rotation_preserves_position_and_child_geometry() {
        let board = r#"(kicad_pcb
  (footprint "Test:Part"
    (layer "F.Cu")
    (at 10 20)
    (property "Reference" "U1" (at 0 -2 0))
    (pad "1" smd rect (at -1 0) (size 1 1))
  )
)"#;
        let (rotated, x, y, old_rotation) =
            update_footprint_transform(board, "U1", None, None, Some(90.0)).unwrap();
        assert_eq!((x, y, old_rotation), (10.0, 20.0, 0.0));
        assert!(rotated.contains("(at 10 20 90)"));
        assert!(rotated.contains("(pad \"1\" smd rect (at -1 0)"));
    }
}

fn requested_references(args: &serde_json::Value) -> Vec<String> {
    args.get("references")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn handle_get_footprint_courtyards(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let requested = requested_references(args);
    let courtyards = ipc!(ctx, args, |c| c.list_footprint_courtyards());
    let filtered: Vec<_> = courtyards
        .into_iter()
        .filter(|c| requested.is_empty() || requested.contains(&c.reference))
        .collect();
    Ok(CallToolResult::json(
        &json!({"coordinate_space":"board_mm","source":"active_kicad_ipc","count":filtered.len(),"courtyards":filtered}),
    ))
}

async fn handle_add_footprint_courtyard_circle(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let diameter = match require_f64(args, "diameter") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let line_width = args
        .get("line_width")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.05);
    let layer = args
        .get("layer")
        .and_then(|v| v.as_str())
        .unwrap_or("F.CrtYd")
        .to_string();
    let refs = ipc!(ctx, args, |c| c.list_footprints());
    let fp = refs
        .into_iter()
        .find(|f| f.reference == reference)
        .ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", reference))?;
    let x = fp.position.x;
    let y = fp.position.y;
    let result_ref = reference.clone();
    let result_layer = layer.clone();
    ipc!(ctx, args, |c| c.add_footprint_circle(
        &reference, &layer, x, y, diameter, line_width
    ));
    Ok(CallToolResult::json(
        &json!({"reference":result_ref,"layer":result_layer,"center":{"x":x,"y":y},"diameter":diameter,"line_width":line_width,"source":"active_kicad_ipc"}),
    ))
}

fn bounds_overlap(
    a: &konnect_ipc::types::IpcBounds,
    b: &konnect_ipc::types::IpcBounds,
    clearance: f64,
) -> bool {
    a.min.x < b.max.x + clearance
        && a.max.x + clearance > b.min.x
        && a.min.y < b.max.y + clearance
        && a.max.y + clearance > b.min.y
}

async fn handle_check_courtyard_overlaps(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let requested = requested_references(args);
    let clearance = args
        .get("clearance")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if clearance < 0.0 {
        return Ok(CallToolResult::error("clearance must be >= 0"));
    }
    let courtyards = ipc!(ctx, args, |c| c.list_footprint_courtyards());
    let selected: Vec<_> = courtyards
        .into_iter()
        .filter(|c| requested.is_empty() || requested.contains(&c.reference))
        .collect();
    let mut overlaps = Vec::new();
    for i in 0..selected.len() {
        for b in &selected[i + 1..] {
            let a = &selected[i];
            if a.reference == b.reference || a.layer != b.layer {
                continue;
            }
            if let (Some(ab), Some(bb)) = (&a.bounds, &b.bounds) {
                if bounds_overlap(ab, bb, clearance) {
                    overlaps.push(json!({"ref1":a.reference,"ref2":b.reference,"layer":a.layer,"bounds1":ab,"bounds2":bb}))
                }
            }
        }
    }
    Ok(CallToolResult::json(
        &json!({"source":"active_kicad_ipc","method":"courtyard_aabb","clearance_mm":clearance,"checked":selected.len(),"overlap_count":overlaps.len(),"overlaps":overlaps}),
    ))
}
