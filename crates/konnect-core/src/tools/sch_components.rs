//! `sch_components` toolset — add, edit, move, rotate, delete schematic symbols.
//!
//! Simple CRUD operations use `konnect_schematic_editor` (cse) for structured
//! round-trip parsing.  Pin coordinate math still delegates to
//! `konnect_sexp::geometry::transform_pin`.

use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{get_path, opt_f64, opt_str, require_f64, require_str, ToolContext, ToolDef};
use konnect_schematic_editor as cse;
use konnect_sexp::{
    geometry::{snap_point, transform_pin},
    schematic::{extract_lib_pins, extract_symbol_instances, pin_endpoint, read_schematic},
    writer::{apply_edits, find_block_with_leading_whitespace, new_uuid, write_atomic, SexpEdit},
    SexpNode,
};
use serde_json::json;

pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "create_schematic",
            "Create a new blank .kicad_sch schematic file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Full path for the new .kicad_sch file" }
                },
                "required": ["path"]
            }),
            |args, ctx| async move { handle_create_schematic(args, ctx).await }
        ),
        tool!(
            "add_schematic_component",
            "Add a symbol from a KiCAD library to the schematic. The symbol is snapped \
             to the 1.27mm schematic grid. Specify position in schematic mm coordinates.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "lib_id": { "type": "string", "description": "Library:Symbol (e.g. 'Device:R')" },
                    "x": { "type": "number", "description": "X position in mm" },
                    "y": { "type": "number", "description": "Y position in mm" },
                    "rotation": { "type": "number", "description": "Rotation in degrees (0/90/180/270)", "default": 0 },
                    "reference": { "type": "string", "description": "Optional override for reference designator" },
                    "value": { "type": "string", "description": "Optional override for value field" }
                },
                "required": ["schematic", "lib_id", "x", "y"]
            }),
            |args, ctx| async move { handle_add_schematic_component(args, ctx).await }
        ),
        tool!(
            "delete_schematic_component",
            "Remove a symbol instance from the schematic by its reference designator.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string", "description": "Reference designator (e.g. 'R1')" }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_delete_schematic_component(args, ctx).await }
        ),
        tool!(
            "edit_schematic_component",
            "Update fields (Reference, Value, Footprint, custom properties) of a symbol instance.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string", "description": "Current reference designator" },
                    "new_reference": { "type": "string", "description": "New reference designator (optional)" },
                    "value": { "type": "string", "description": "New value (optional)" },
                    "footprint": { "type": "string", "description": "New footprint (optional)" },
                    "datasheet": { "type": "string", "description": "New datasheet URL (optional)" },
                    "reference_x": { "type": "number", "description": "Reference field X position (optional)" },
                    "reference_y": { "type": "number", "description": "Reference field Y position (optional)" },
                    "reference_rotation": { "type": "number", "description": "Reference field rotation (optional)" },
                    "value_x": { "type": "number", "description": "Value field X position (optional)" },
                    "value_y": { "type": "number", "description": "Value field Y position (optional)" },
                    "value_rotation": { "type": "number", "description": "Value field rotation (optional)" },
                    "fields": {
                        "type": "object",
                        "description": "Additional property fields to set as key:value pairs"
                    }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_edit_schematic_component(args, ctx).await }
        ),
        tool!(
            "get_schematic_component",
            "Get all properties, position, and pin locations for a symbol instance.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_get_schematic_component(args, ctx).await }
        ),
        tool!(
            "list_schematic_components",
            "List all symbol instances in a schematic with their positions, values, \
             footprints, and pin locations.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" }
                },
                "required": ["schematic"]
            }),
            |args, ctx| async move { handle_list_schematic_components(args, ctx).await }
        ),
        tool!(
            "move_schematic_component",
            "Move a symbol to a new position. Does NOT adjust connected wires.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" },
                    "x": { "type": "number", "description": "New X position in mm" },
                    "y": { "type": "number", "description": "New Y position in mm" }
                },
                "required": ["schematic", "reference", "x", "y"]
            }),
            |args, ctx| async move { handle_move_schematic_component(args, ctx).await }
        ),
        tool!(
            "rotate_schematic_component",
            "Rotate a symbol by setting its absolute rotation angle (0/90/180/270).",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" },
                    "rotation": { "type": "number", "description": "Absolute rotation in degrees" }
                },
                "required": ["schematic", "reference", "rotation"]
            }),
            |args, ctx| async move { handle_rotate_schematic_component(args, ctx).await }
        ),
        tool!(
            "move_connected",
            "Move a symbol and stretch/shrink connected wire stubs to preserve connections.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" },
                    "x": { "type": "number" },
                    "y": { "type": "number" }
                },
                "required": ["schematic", "reference", "x", "y"]
            }),
            |args, ctx| async move { handle_move_connected(args, ctx).await }
        ),
        tool!(
            "move_region",
            "Move all symbols within a bounding box by a given offset.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "x1": { "type": "number", "description": "Region bounding box min X" },
                    "y1": { "type": "number", "description": "Region bounding box min Y" },
                    "x2": { "type": "number", "description": "Region bounding box max X" },
                    "y2": { "type": "number", "description": "Region bounding box max Y" },
                    "dx": { "type": "number", "description": "X offset to move by" },
                    "dy": { "type": "number", "description": "Y offset to move by" }
                },
                "required": ["schematic", "x1", "y1", "x2", "y2", "dx", "dy"]
            }),
            |args, ctx| async move { handle_move_region(args, ctx).await }
        ),
        tool!(
            "annotate_schematic",
            "Run kicad-cli to auto-assign reference designators (R? → R1, U? → U1, etc.).",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" }
                },
                "required": ["schematic"]
            }),
            |args, ctx| async move { handle_annotate_schematic(args, ctx).await }
        ),
        tool!(
            "get_schematic_pin_locations",
            "Get the exact schematic-space (X,Y) coordinates of every pin on a symbol, \
             accounting for rotation and mirroring. Uses the canonical pin transform.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_get_schematic_pin_locations(args, ctx).await }
        ),
        tool!(
            "batch_get_schematic_pin_locations",
            "Get pin locations for multiple components in a single file read.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "references": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of reference designators"
                    }
                },
                "required": ["schematic", "references"]
            }),
            |args, ctx| async move { handle_batch_get_pin_locations(args, ctx).await }
        ),
        tool!(
            "get_schematic_symbol_bounds",
            "Return the transformed symbol body bounds and estimated Reference/Value text bounds without modifying the schematic.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_get_symbol_bounds(args, ctx).await }
        ),
        tool!(
            "check_schematic_field_spacing",
            "Audit Reference/Value spacing against actual transformed symbol body graphics. This tool is read-only.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "max_clearance": { "type": "number", "default": 1.27 },
                    "min_clearance": { "type": "number", "default": 0.20 }
                },
                "required": ["schematic"]
            }),
            |args, ctx| async move { handle_check_field_spacing(args, ctx).await }
        ),
        tool!(
            "add_component_annotation",
            "Add a custom property (annotation) to a symbol instance in the schematic.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "reference": { "type": "string", "description": "Component reference designator (e.g. 'R1')" },
                    "key": { "type": "string", "description": "Property name" },
                    "value": { "type": "string", "description": "Property value" }
                },
                "required": ["schematic", "reference", "key", "value"]
            }),
            |args, ctx| async move { handle_add_component_annotation(args, ctx).await }
        ),
        tool!(
            "group_components",
            "Add a group property to multiple components in the schematic.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "references": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of reference designators to group"
                    },
                    "group_name": { "type": "string", "description": "Group name to assign" }
                },
                "required": ["schematic", "references", "group_name"]
            }),
            |args, ctx| async move { handle_group_components(args, ctx).await }
        ),
        tool!(
            "replace_component",
            "Replace a component's lib_id with a new library symbol (swap the component type).",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "reference": { "type": "string", "description": "Component reference designator (e.g. 'U1')" },
                    "new_lib_id": { "type": "string", "description": "New Library:Symbol identifier (e.g. 'Device:C')" },
                    "source_schematic": { "type": "string", "description": "Optional verified schematic whose complete embedded symbol definition should be reused" }
                },
                "required": ["schematic", "reference", "new_lib_id"]
            }),
            |args, ctx| async move { handle_replace_component(args, ctx).await }
        ),
        tool!(
            "get_schematic_view",
            "Render the schematic to a PNG image (base64-encoded) via kicad-cli.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string" }
                },
                "required": ["schematic"]
            }),
            |args, ctx| async move { handle_get_schematic_view(args, ctx).await }
        ),
    ]
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn handle_create_schematic(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let path = get_path(args, "path")?;
    // Build a minimal valid schematic and save via cse's atomic writer
    let root_uuid = uuid::Uuid::new_v4().to_string();
    let template = format!(
        "(kicad_sch\n\t(version 20250610)\n\t(generator \"konnect\")\n\t(generator_version \"10.0\")\n\t(uuid \"{root_uuid}\")\n\t(paper \"A4\")\n\t(lib_symbols\n\t)\n)\n"
    );
    // Write the template then immediately load/save through cse so the file
    // is normalised to cse's writer output format.
    write_atomic(&path, &template)?;
    let sch = cse::Schematic::load(&path)?;
    sch.overwrite()?;
    Ok(CallToolResult::json(
        &json!({ "created": path.display().to_string() }),
    ))
}

async fn handle_add_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let lib_id = match require_str(args, "lib_id") {
        Ok(s) => s.to_string(),
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
    let rotation = opt_f64(args, "rotation").unwrap_or(0.0);
    let reference = opt_str(args, "reference");
    let value = opt_str(args, "value");

    // Snap to 1.27mm grid
    let (x, y) = snap_point(x, y, 1.27);

    let ref_str = reference.unwrap_or("?");
    let val_str = value.unwrap_or(lib_id.split(':').next_back().unwrap_or("?"));

    // Load via konnect-schematic-editor
    let mut sch = cse::Schematic::load(&sch_path)?;
    let root_uuid = sch.uuid.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "Schematic '{}' has no root UUID; refusing to create an unsafe symbol instance",
            sch_path.display()
        )
    })?;
    let project_name = sch_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| anyhow::anyhow!("Schematic path has no valid file stem"))?;
    let instance_path = format!("/{root_uuid}");

    // Embed the library symbol definition
    cse::library::ensure_lib_symbol(&mut sch, &lib_id);

    // Build the Symbol struct
    let mut sym = cse::Symbol::new(&lib_id, x, y);
    sym.at.rotation = Some(rotation);

    // Helper: build an effects sub-node  (font (size 1.27 1.27))  with optional (hide yes)
    let effects_node = |hide: bool| -> cse::sexp::SexpNode {
        let font = cse::sexp::SexpNode::List(vec![
            cse::sexp::atom("font"),
            cse::sexp::SexpNode::List(vec![
                cse::sexp::atom("size"),
                cse::sexp::atom("1.27"),
                cse::sexp::atom("1.27"),
            ]),
        ]);
        let mut children = vec![cse::sexp::atom("effects"), font];
        if hide {
            children.push(cse::sexp::SexpNode::List(vec![
                cse::sexp::atom("hide"),
                cse::sexp::atom("yes"),
            ]));
        }
        cse::sexp::SexpNode::List(children)
    };

    // Helper: build an (at X Y ROT) sub-node
    let at_node = |px: f64, py: f64, rot: f64| -> cse::sexp::SexpNode {
        cse::sexp::SexpNode::List(vec![
            cse::sexp::atom("at"),
            cse::sexp::atom(cse::types::fmt_f64(px)),
            cse::sexp::atom(cse::types::fmt_f64(py)),
            cse::sexp::atom(cse::types::fmt_f64(rot)),
        ])
    };

    // Offset Reference above component, Value below
    let ref_y = y - 3.81;
    let val_y = y + 3.81;

    // Reference property
    let mut ref_prop = cse::Property::new("Reference", ref_str);
    ref_prop.sub_nodes.push(at_node(x, ref_y, 0.0));
    ref_prop.sub_nodes.push(effects_node(false));
    sym.properties.push(ref_prop);

    // Value property
    let mut val_prop = cse::Property::new("Value", val_str);
    val_prop.sub_nodes.push(at_node(x, val_y, 0.0));
    val_prop.sub_nodes.push(effects_node(false));
    sym.properties.push(val_prop);

    // Footprint property (hidden)
    let mut fp_prop = cse::Property::new("Footprint", "");
    fp_prop.sub_nodes.push(at_node(x, y, 0.0));
    fp_prop.sub_nodes.push(effects_node(true));
    sym.properties.push(fp_prop);

    // Datasheet property (hidden)
    let mut ds_prop = cse::Property::new("Datasheet", "");
    ds_prop.sub_nodes.push(at_node(x, y, 0.0));
    ds_prop.sub_nodes.push(effects_node(true));
    sym.properties.push(ds_prop);

    // Every placed symbol pin needs its own KIID.  Library pins do not carry
    // instance UUIDs; KiCad expects them as child nodes of the placed symbol.
    // Missing pin UUIDs are particularly dangerous because the schematic can
    // still load, then crash in KIID::operator< when autosave runs after an
    // interactive edit.
    let pin_numbers = cse::library::resolve_lib_symbol_pin_numbers(&lib_id);
    if pin_numbers.is_empty() {
        return Ok(CallToolResult::error(format!(
            "Could not resolve any pins for library symbol '{}'; refusing to create an unsafe symbol instance",
            lib_id
        )));
    }
    for number in &pin_numbers {
        sym.raw_sub_nodes.push(cse::sexp::SexpNode::List(vec![
            cse::sexp::atom("pin"),
            cse::sexp::qstr(number),
            cse::sexp::SexpNode::List(vec![
                cse::sexp::atom("uuid"),
                cse::sexp::qstr(uuid::Uuid::new_v4().to_string()),
            ]),
        ]));
    }

    // Instances node
    let instances = cse::sexp::SexpNode::List(vec![
        cse::sexp::atom("instances"),
        cse::sexp::SexpNode::List(vec![
            cse::sexp::atom("project"),
            cse::sexp::qstr(project_name),
            cse::sexp::SexpNode::List(vec![
                cse::sexp::atom("path"),
                cse::sexp::qstr(instance_path),
                cse::sexp::SexpNode::List(vec![
                    cse::sexp::atom("reference"),
                    cse::sexp::qstr(ref_str),
                ]),
                cse::sexp::SexpNode::List(vec![cse::sexp::atom("unit"), cse::sexp::atom("1")]),
            ]),
        ]),
    ]);
    sym.raw_sub_nodes.push(instances);

    let uuid = sym.uuid.clone();
    sch.add_symbol(sym);
    sch.overwrite()?;

    Ok(CallToolResult::json(&json!({
        "added": lib_id,
        "reference": ref_str,
        "value": val_str,
        "x": x, "y": y,
        "uuid": uuid,
        "pin_uuid_count": pin_numbers.len()
    })))
}

async fn handle_delete_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };

    let mut sch = cse::Schematic::load(&sch_path)?;

    match sch.symbols.remove_by_reference(&reference) {
        Some(_) => {
            sch.overwrite()?;
            Ok(CallToolResult::json(&json!({ "deleted": reference })))
        }
        None => Ok(CallToolResult::error(format!(
            "Component '{}' not found in schematic",
            reference
        ))),
    }
}

async fn handle_edit_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };

    let mut content = std::fs::read_to_string(&sch_path)?;
    let mut changed = Vec::new();

    // Helper: update a property field value in-place
    let update_field = |content: &str, ref_: &str, field: &str, new_val: &str| -> (String, bool) {
        // Pattern: (property "FieldName" "OldValue"
        //           surrounded by the enclosing symbol block for 'ref_'
        // Simple approach: find the reference location, then within that symbol block
        // update the named property.
        let ref_search = format!(r#"(property "Reference" "{ref_}""#);
        let ref_pos = match content.find(&ref_search) {
            Some(p) => p,
            None => return (content.to_string(), false),
        };
        // Find the symbol block around this reference
        let before = &content[..ref_pos];
        let sym_start = match ["\n  (symbol", "\n\t(symbol"]
            .iter()
            .filter_map(|pattern| before.rfind(pattern))
            .max()
        {
            Some(p) => p + 1,
            None => return (content.to_string(), false),
        };
        let (_, sym_end) = match find_block_with_leading_whitespace(content, sym_start) {
            Some(r) => r,
            None => return (content.to_string(), false),
        };
        let sym_block = &content[sym_start..sym_end];
        let field_search = format!(r#"(property "{field}" ""#);
        let field_offset = match sym_block.find(&field_search) {
            Some(o) => sym_start + o + field_search.len(),
            None => return (content.to_string(), false),
        };
        // Find the closing quote of the current value
        let val_end = match content[field_offset..].find('"') {
            Some(o) => field_offset + o,
            None => return (content.to_string(), false),
        };
        let new_content = format!(
            "{}{}{}",
            &content[..field_offset],
            new_val,
            &content[val_end..]
        );
        (new_content, true)
    };

    if let Some(new_ref) = opt_str(args, "new_reference") {
        let (c, ok) = update_field(&content, &reference, "Reference", new_ref);
        if ok {
            content = c;
            changed.push(format!("Reference → {}", new_ref));
        }
    }
    if let Some(val) = opt_str(args, "value") {
        let (c, ok) = update_field(&content, &reference, "Value", val);
        if ok {
            content = c;
            changed.push(format!("Value → {}", val));
        }
    }
    if let Some(fp) = opt_str(args, "footprint") {
        let (c, ok) = update_field(&content, &reference, "Footprint", fp);
        if ok {
            content = c;
            changed.push(format!("Footprint → {}", fp));
        }
    }
    if let Some(ds) = opt_str(args, "datasheet") {
        let (c, ok) = update_field(&content, &reference, "Datasheet", ds);
        if ok {
            content = c;
            changed.push(format!("Datasheet → {}", ds));
        }
    }

    write_atomic(&sch_path, &content)?;

    let has_property_layout = [
        "reference_x",
        "reference_y",
        "reference_rotation",
        "value_x",
        "value_y",
        "value_rotation",
    ]
    .iter()
    .any(|key| args.get(*key).and_then(|value| value.as_f64()).is_some());

    if has_property_layout {
        let active_reference = opt_str(args, "new_reference").unwrap_or(&reference);
        let mut sch = cse::Schematic::load(&sch_path)?;
        let sym = sch
            .symbols
            .by_reference_mut(active_reference)
            .ok_or_else(|| anyhow::anyhow!("Component '{}' not found", active_reference))?;
        let (symbol_x, symbol_y) = sym.position();

        let mut update_layout = |name: &str, x_key: &str, y_key: &str, rotation_key: &str| {
            if ![x_key, y_key, rotation_key]
                .iter()
                .any(|key| args.get(*key).and_then(|value| value.as_f64()).is_some())
            {
                return;
            }
            let Some(property) = sym
                .properties
                .iter_mut()
                .find(|property| property.name == name)
            else {
                return;
            };
            let existing = property
                .sub_nodes
                .iter()
                .find(|node| node.tag() == Some("at"))
                .map(|node| node.scalar_args())
                .unwrap_or_default();
            let existing_x = existing
                .first()
                .and_then(|value| value.parse().ok())
                .unwrap_or(symbol_x);
            let existing_y = existing
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(symbol_y);
            let existing_rotation = existing
                .get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
            let x = opt_f64(args, x_key).unwrap_or(existing_x);
            let y = opt_f64(args, y_key).unwrap_or(existing_y);
            let rotation = opt_f64(args, rotation_key).unwrap_or(existing_rotation);
            let at = cse::sexp::SexpNode::List(vec![
                cse::sexp::atom("at"),
                cse::sexp::atom(cse::types::fmt_f64(x)),
                cse::sexp::atom(cse::types::fmt_f64(y)),
                cse::sexp::atom(cse::types::fmt_f64(rotation)),
            ]);
            if let Some(existing) = property
                .sub_nodes
                .iter_mut()
                .find(|node| node.tag() == Some("at"))
            {
                *existing = at;
            } else {
                property.sub_nodes.insert(0, at);
            }
            changed.push(format!("{} layout", name));
        };

        update_layout(
            "Reference",
            "reference_x",
            "reference_y",
            "reference_rotation",
        );
        update_layout("Value", "value_x", "value_y", "value_rotation");
        sch.overwrite()?;
    }

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "changes": changed
    })))
}

async fn handle_get_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };

    let sch = cse::Schematic::load(&sch_path)?;

    match sch.symbols.by_reference(&reference) {
        Some(sym) => {
            let (x, y) = sym.position();
            let rotation = sym.at.rotation.unwrap_or(0.0);
            let mirror = sym.mirror.as_deref().unwrap_or("");
            Ok(CallToolResult::json(&json!({
                "reference": sym.reference().unwrap_or("?"),
                "value": sym.value_str().unwrap_or(""),
                "footprint": sym.footprint().unwrap_or(""),
                "lib_id": sym.lib_id,
                "x": x,
                "y": y,
                "rotation": rotation,
                "mirror_x": mirror.contains('x'),
                "mirror_y": mirror.contains('y'),
                "uuid": sym.uuid
            })))
        }
        None => Ok(CallToolResult::error(format!(
            "Component '{}' not found",
            reference
        ))),
    }
}

async fn handle_list_schematic_components(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;

    let items: Vec<serde_json::Value> = sch
        .symbols
        .iter()
        .map(|sym| {
            let (x, y) = sym.position();
            let rotation = sym.at.rotation.unwrap_or(0.0);
            let mirror = sym.mirror.as_deref().unwrap_or("");
            json!({
                "reference": sym.reference().unwrap_or("?"),
                "value": sym.value_str().unwrap_or(""),
                "footprint": sym.footprint().unwrap_or(""),
                "lib_id": sym.lib_id,
                "x": x,
                "y": y,
                "rotation": rotation,
                "mirror_x": mirror.contains('x'),
                "mirror_y": mirror.contains('y')
            })
        })
        .collect();

    Ok(CallToolResult::json(&json!({
        "count": items.len(),
        "components": items
    })))
}

async fn handle_move_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };
    let new_x = match require_f64(args, "x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let new_y = match require_f64(args, "y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let (new_x, new_y) = snap_point(new_x, new_y, 1.27);

    let mut sch = cse::Schematic::load(&sch_path)?;

    match sch.symbols.by_reference_mut(&reference) {
        Some(sym) => {
            sym.move_to(new_x, new_y);
            sch.overwrite()?;
            Ok(CallToolResult::json(
                &json!({ "moved": reference, "x": new_x, "y": new_y }),
            ))
        }
        None => Err(anyhow::anyhow!("Component '{}' not found", reference)),
    }
}

async fn handle_rotate_schematic_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };
    let rotation = match require_f64(args, "rotation") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    let mut sch = cse::Schematic::load(&sch_path)?;

    match sch.symbols.by_reference_mut(&reference) {
        Some(sym) => {
            sym.set_rotation(rotation);
            sch.overwrite()?;
            Ok(CallToolResult::json(
                &json!({ "rotated": reference, "rotation": rotation }),
            ))
        }
        None => Err(anyhow::anyhow!("Component '{}' not found", reference)),
    }
}

async fn handle_move_connected(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    // For now: delegate to simple move. Wire adjustment is a Phase 2 enhancement.
    handle_move_schematic_component(args, ctx).await
}

async fn handle_move_region(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let x1 = match require_f64(args, "x1") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y1 = match require_f64(args, "y1") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let x2 = match require_f64(args, "x2") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y2 = match require_f64(args, "y2") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let dx = match require_f64(args, "dx") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let dy = match require_f64(args, "dy") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };

    let mut sch = cse::Schematic::load(&sch_path)?;

    // Collect references of symbols within the bounding box
    let refs_to_move: Vec<String> = sch
        .symbols
        .within_rectangle(x1, y1, x2, y2)
        .iter()
        .filter_map(|s| s.reference().map(String::from))
        .collect();

    let mut moved = Vec::new();
    for reference in &refs_to_move {
        if let Some(sym) = sch.symbols.by_reference_mut(reference) {
            let (ox, oy) = sym.position();
            let (nx, ny) = snap_point(ox + dx, oy + dy, 1.27);
            sym.move_to(nx, ny);
            moved.push(reference.clone());
        }
    }

    sch.overwrite()?;

    Ok(CallToolResult::json(&json!({
        "moved_count": moved.len(),
        "moved": moved
    })))
}

async fn handle_annotate_schematic(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    crate::tools::cli::annotate_schematic(&ctx.config.kicad_cli, &sch_path).await?;
    Ok(CallToolResult::text("Annotation complete."))
}

async fn handle_get_schematic_pin_locations(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };

    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let inst = match instances.iter().find(|i| i.reference == reference) {
        Some(i) => i,
        None => {
            return Ok(CallToolResult::error(format!(
                "Component '{}' not found",
                reference
            )))
        }
    };

    // Find the library symbol definition within the schematic's lib_symbols section
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();
    let lib_sym = resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id);

    let pins: Vec<serde_json::Value> = if let Some(sym) = lib_sym {
        let lib_pins = extract_lib_pins(sym);
        let t = inst.pin_transform();
        lib_pins
            .iter()
            .map(|p| {
                let (sx, sy) = pin_endpoint(p, t);
                json!({
                    "number": p.number,
                    "name": p.name,
                    "x": sx,
                    "y": sy
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "component_x": inst.x,
        "component_y": inst.y,
        "rotation": inst.rotation,
        "pins": pins
    })))
}

async fn handle_batch_get_pin_locations(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let refs = args["references"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let (_, tree) = read_schematic(&sch_path)?; // single read
    let instances = extract_symbol_instances(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();

    let results: Vec<serde_json::Value> = refs
        .iter()
        .map(|reference| {
            let inst = match instances.iter().find(|i| &i.reference == reference) {
                Some(i) => i,
                None => return json!({ "reference": reference, "error": "not found" }),
            };
            let lib_sym = resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id);
            let pins: Vec<serde_json::Value> = if let Some(sym) = lib_sym {
                let t = inst.pin_transform();
                extract_lib_pins(sym)
                    .iter()
                    .map(|p| {
                        let (sx, sy) = pin_endpoint(p, t);
                        json!({ "number": p.number, "name": p.name, "x": sx, "y": sy })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            json!({ "reference": reference, "x": inst.x, "y": inst.y, "pins": pins })
        })
        .collect();

    Ok(CallToolResult::json(&json!({ "components": results })))
}

#[derive(Clone, Copy, Debug)]
struct SchBounds {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl SchBounds {
    fn empty() -> Self {
        Self {
            left: f64::INFINITY,
            right: f64::NEG_INFINITY,
            top: f64::INFINITY,
            bottom: f64::NEG_INFINITY,
        }
    }

    fn include(&mut self, x: f64, y: f64) {
        self.left = self.left.min(x);
        self.right = self.right.max(x);
        self.top = self.top.min(y);
        self.bottom = self.bottom.max(y);
    }

    fn valid(&self) -> bool {
        self.left.is_finite()
            && self.right.is_finite()
            && self.top.is_finite()
            && self.bottom.is_finite()
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "left": self.left, "right": self.right,
            "top": self.top, "bottom": self.bottom,
            "width": self.right - self.left,
            "height": self.bottom - self.top
        })
    }
}

fn collect_graphic_points(node: &SexpNode, points: &mut Vec<(f64, f64)>) {
    match node.head() {
        Some("rectangle") => {
            for tag in ["start", "end"] {
                if let Some(point) = node.find(tag) {
                    if let (Some(x), Some(y)) = (point.get_f64(1), point.get_f64(2)) {
                        points.push((x, y));
                    }
                }
            }
        }
        Some("circle") => {
            if let (Some(center), Some(radius)) = (node.find("center"), node.find_f64("radius")) {
                if let (Some(x), Some(y)) = (center.get_f64(1), center.get_f64(2)) {
                    points.extend([
                        (x - radius, y),
                        (x + radius, y),
                        (x, y - radius),
                        (x, y + radius),
                    ]);
                }
            }
        }
        Some("arc") => {
            for tag in ["start", "mid", "end"] {
                if let Some(point) = node.find(tag) {
                    if let (Some(x), Some(y)) = (point.get_f64(1), point.get_f64(2)) {
                        points.push((x, y));
                    }
                }
            }
        }
        Some("polyline") | Some("bezier") => {
            if let Some(pts) = node.find("pts") {
                for point in pts.find_all("xy") {
                    if let (Some(x), Some(y)) = (point.get_f64(1), point.get_f64(2)) {
                        points.push((x, y));
                    }
                }
            }
        }
        _ => {}
    }

    if let Some(children) = node.children() {
        for child in children {
            // Pins and fields are intentionally excluded from body geometry.
            if !matches!(child.head(), Some("pin") | Some("property") | Some("text")) {
                collect_graphic_points(child, points);
            }
        }
    }
}

fn transformed_body_bounds(
    sym: &SexpNode,
    inst: &konnect_sexp::schematic::SymbolInstance,
) -> Option<SchBounds> {
    let mut local_points = Vec::new();
    collect_graphic_points(sym, &mut local_points);
    let transform = inst.pin_transform();
    let mut bounds = SchBounds::empty();
    for (x, y) in local_points {
        let (world_x, world_y) = transform_pin(x, y, transform);
        bounds.include(world_x, world_y);
    }
    bounds.valid().then_some(bounds)
}

fn rendered_field_rotation(symbol_rotation: f64, stored_rotation: f64) -> f64 {
    (symbol_rotation - stored_rotation).rem_euclid(180.0)
}

fn property_bounds(
    instance_node: &SexpNode,
    name: &str,
    symbol_rotation: f64,
) -> Option<(SchBounds, f64, f64, f64, f64)> {
    let property = instance_node
        .find_all("property")
        .into_iter()
        .find(|node| node.get(1).and_then(SexpNode::as_str) == Some(name))?;
    let at = property.find("at")?;
    let x = at.get_f64(1)?;
    let y = at.get_f64(2)?;
    let rotation = at.get_f64(3).unwrap_or(0.0).rem_euclid(360.0);
    // KiCad stores symbol-property rotation in the symbol's rotated frame.
    // Convert it to the rendered page orientation before estimating glyph bounds.
    let rendered_rotation = rendered_field_rotation(symbol_rotation, rotation);
    let text = property.get(2).and_then(SexpNode::as_str).unwrap_or("");
    let effects = property.find("effects");
    let hidden = property
        .find("hide")
        .or_else(|| effects.and_then(|node| node.find("hide")))
        .and_then(|node| node.get(1))
        .and_then(SexpNode::as_str)
        .is_some_and(|value| value == "yes");
    if hidden {
        return None;
    }
    let font_size = effects
        .and_then(|effects| effects.find("font"))
        .and_then(|font| font.find("size"));
    let font_x = font_size.and_then(|size| size.get_f64(1)).unwrap_or(1.27);
    let font_y = font_size.and_then(|size| size.get_f64(2)).unwrap_or(font_x);
    let justify = effects
        .and_then(|effects| effects.find("justify"))
        .and_then(|justify| justify.get(1))
        .and_then(SexpNode::as_str)
        .unwrap_or("center");

    // KiCad's stroke font is variable width. 0.65 em per character is a
    // conservative estimate for spacing audits; body bounds remain exact.
    let width = (text.chars().count().max(1) as f64) * font_x * 0.65;
    let height = font_y;
    let (local_left, local_right) = match justify {
        "left" => (0.0, width),
        "right" => (-width, 0.0),
        _ => (-width / 2.0, width / 2.0),
    };
    let radians = rendered_rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let mut bounds = SchBounds::empty();
    for (local_x, local_y) in [
        (local_left, -height / 2.0),
        (local_right, -height / 2.0),
        (local_left, height / 2.0),
        (local_right, height / 2.0),
    ] {
        bounds.include(
            x + local_x * cos - local_y * sin,
            y + local_x * sin + local_y * cos,
        );
    }
    Some((bounds, x, y, rotation, rendered_rotation))
}

fn bounds_gap(first: SchBounds, second: SchBounds) -> (f64, bool) {
    let dx = if first.right < second.left {
        second.left - first.right
    } else if second.right < first.left {
        first.left - second.right
    } else {
        0.0
    };
    let dy = if first.bottom < second.top {
        second.top - first.bottom
    } else if second.bottom < first.top {
        first.top - second.bottom
    } else {
        0.0
    };
    let overlaps = dx == 0.0 && dy == 0.0;
    ((dx * dx + dy * dy).sqrt(), overlaps)
}

fn edge_pin_corridor(body: SchBounds, pin_x: f64, pin_y: f64, clearance: f64) -> Option<SchBounds> {
    if pin_y > body.bottom && pin_x >= body.left && pin_x <= body.right {
        Some(SchBounds {
            left: pin_x - clearance,
            right: pin_x + clearance,
            top: body.bottom,
            bottom: pin_y,
        })
    } else if pin_y < body.top && pin_x >= body.left && pin_x <= body.right {
        Some(SchBounds {
            left: pin_x - clearance,
            right: pin_x + clearance,
            top: pin_y,
            bottom: body.top,
        })
    } else if pin_x < body.left && pin_y >= body.top && pin_y <= body.bottom {
        Some(SchBounds {
            left: pin_x,
            right: body.left,
            top: pin_y - clearance,
            bottom: pin_y + clearance,
        })
    } else if pin_x > body.right && pin_y >= body.top && pin_y <= body.bottom {
        Some(SchBounds {
            left: body.right,
            right: pin_x,
            top: pin_y - clearance,
            bottom: pin_y + clearance,
        })
    } else {
        None
    }
}

fn find_instance_node<'a>(tree: &'a SexpNode, reference: &str) -> Option<&'a SexpNode> {
    tree.find_all("symbol").into_iter().find(|node| {
        node.find_all("property").into_iter().any(|property| {
            property.get(1).and_then(SexpNode::as_str) == Some("Reference")
                && property.get(2).and_then(SexpNode::as_str) == Some(reference)
        })
    })
}

fn symbol_bounds_result(
    tree: &SexpNode,
    instances: &[konnect_sexp::schematic::SymbolInstance],
    lib_syms: &[&SexpNode],
    reference: &str,
) -> anyhow::Result<serde_json::Value> {
    let inst = instances
        .iter()
        .find(|instance| instance.reference == reference)
        .ok_or_else(|| anyhow::anyhow!("Component '{}' not found", reference))?;
    let lib_sym = resolve_embedded_pin_symbol(lib_syms, &inst.lib_id)
        .ok_or_else(|| anyhow::anyhow!("Library symbol '{}' not found", inst.lib_id))?;
    let body = transformed_body_bounds(lib_sym, inst)
        .ok_or_else(|| anyhow::anyhow!("Symbol '{}' has no supported body graphics", reference))?;
    let instance_node = find_instance_node(tree, reference)
        .ok_or_else(|| anyhow::anyhow!("Instance node '{}' not found", reference))?;

    let field_json = |name: &str| {
        property_bounds(instance_node, name, inst.rotation).map(
            |(bounds, x, y, rotation, rendered_rotation)| {
            let (gap, overlaps) = bounds_gap(body, bounds);
            json!({
                "anchor": {"x": x, "y": y, "rotation": rotation, "rendered_rotation": rendered_rotation},
                "bounds": bounds.json(),
                "body_clearance": gap,
                "overlaps_body": overlaps,
                "bounds_are_estimated": true
            })
        })
    };
    let field_to_field = match (
        property_bounds(instance_node, "Reference", inst.rotation),
        property_bounds(instance_node, "Value", inst.rotation),
    ) {
        (Some((reference_bounds, ..)), Some((value_bounds, ..))) => {
            let (gap, overlaps) = bounds_gap(reference_bounds, value_bounds);
            Some(json!({"clearance": gap, "overlaps": overlaps}))
        }
        _ => None,
    };

    Ok(json!({
        "reference": reference,
        "lib_id": inst.lib_id,
        "origin": {"x": inst.x, "y": inst.y},
        "rotation": inst.rotation,
        "mirror_x": inst.mirror_x,
        "mirror_y": inst.mirror_y,
        "body": body.json(),
        "reference_field": field_json("Reference"),
        "value_field": field_json("Value"),
        "field_to_field": field_to_field
    }))
}

async fn handle_get_symbol_bounds(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|node| node.find_all("symbol"))
        .unwrap_or_default();
    Ok(CallToolResult::json(&symbol_bounds_result(
        &tree, &instances, &lib_syms, reference,
    )?))
}

async fn handle_check_field_spacing(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let max_clearance = args["max_clearance"].as_f64().unwrap_or(1.27);
    let min_clearance = args["min_clearance"].as_f64().unwrap_or(0.20);
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|node| node.find_all("symbol"))
        .unwrap_or_default();
    let mut issues = Vec::new();
    let mut checked = 0usize;

    for instance in &instances {
        let Ok(result) = symbol_bounds_result(&tree, &instances, &lib_syms, &instance.reference)
        else {
            continue;
        };
        checked += 1;
        for field_name in ["reference_field", "value_field"] {
            let Some(field) = result.get(field_name) else {
                continue;
            };
            let clearance = field["body_clearance"].as_f64().unwrap_or(f64::INFINITY);
            let overlaps = field["overlaps_body"].as_bool().unwrap_or(false);
            if overlaps || clearance < min_clearance || clearance > max_clearance {
                issues.push(json!({
                    "reference": instance.reference,
                    "field": if field_name == "reference_field" {"Reference"} else {"Value"},
                    "body_clearance": clearance,
                    "overlaps_body": overlaps,
                    "reason": if overlaps || clearance < min_clearance {"too_close_or_overlapping"} else {"too_far"},
                    "anchor": field["anchor"],
                    "body": result["body"]
                }));
            }
        }
        if let Some(field_spacing) = result.get("field_to_field") {
            let clearance = field_spacing["clearance"].as_f64().unwrap_or(f64::INFINITY);
            let overlaps = field_spacing["overlaps"].as_bool().unwrap_or(false);
            if overlaps || clearance < min_clearance {
                issues.push(json!({
                    "reference": instance.reference,
                    "field": "Reference+Value",
                    "field_clearance": clearance,
                    "fields_overlap": overlaps,
                    "reason": "fields_too_close_or_overlapping",
                    "reference_field": result["reference_field"],
                    "value_field": result["value_field"],
                    "body": result["body"]
                }));
            }
        }
        let body = &result["body"];
        let is_horizontal_resistor = result["lib_id"].as_str() == Some("Device:R")
            && body["width"].as_f64().unwrap_or(0.0)
                > body["height"].as_f64().unwrap_or(f64::INFINITY);
        if is_horizontal_resistor {
            let body_left = body["left"].as_f64().unwrap_or(0.0);
            let body_right = body["right"].as_f64().unwrap_or(0.0);
            let body_top = body["top"].as_f64().unwrap_or(0.0);
            let body_bottom = body["bottom"].as_f64().unwrap_or(0.0);
            let body_center_x = (body_left + body_right) / 2.0;
            for (field_name, expected_side) in
                [("reference_field", "above"), ("value_field", "below")]
            {
                let Some(field) = result.get(field_name) else {
                    continue;
                };
                let rendered_rotation = field["anchor"]["rendered_rotation"]
                    .as_f64()
                    .unwrap_or(f64::NAN);
                let field_center_x = field["anchor"]["x"].as_f64().unwrap_or(f64::NAN);
                let side_ok = if expected_side == "above" {
                    field["bounds"]["bottom"].as_f64().unwrap_or(f64::INFINITY) < body_top
                } else {
                    field["bounds"]["top"].as_f64().unwrap_or(f64::NEG_INFINITY) > body_bottom
                };
                let rotation_ok = rendered_rotation.abs() < 1e-6;
                let centered = (field_center_x - body_center_x).abs() <= 1.27;
                if !rotation_ok || !side_ok || !centered {
                    issues.push(json!({
                        "reference": instance.reference,
                        "field": if field_name == "reference_field" {"Reference"} else {"Value"},
                        "reason": "horizontal_resistor_field_layout",
                        "expected": {"rendered_rotation": 0, "side": expected_side, "center_x": body_center_x, "center_tolerance": 1.27},
                        "actual": field,
                        "rotation_ok": rotation_ok,
                        "side_ok": side_ok,
                        "centered": centered,
                        "body": body
                    }));
                }
            }
        }
        let is_large_connector = result["lib_id"]
            .as_str()
            .is_some_and(|lib_id| lib_id.starts_with("Connector:"))
            && body["width"].as_f64().unwrap_or(0.0) >= 10.0
            && body["height"].as_f64().unwrap_or(0.0) >= 10.0;
        if is_large_connector {
            let body_left = body["left"].as_f64().unwrap_or(0.0);
            let body_right = body["right"].as_f64().unwrap_or(0.0);
            let body_top = body["top"].as_f64().unwrap_or(0.0);
            let body_bottom = body["bottom"].as_f64().unwrap_or(0.0);
            for (field_name, expected_side) in
                [("reference_field", "upper_left"), ("value_field", "below")]
            {
                let Some(field) = result.get(field_name) else {
                    continue;
                };
                let rotation_ok = field["anchor"]["rendered_rotation"]
                    .as_f64()
                    .is_some_and(|rotation| rotation.abs() < 1e-6);
                let anchor_x = field["anchor"]["x"].as_f64().unwrap_or(f64::NAN);
                let side_ok = if expected_side == "upper_left" {
                    field["bounds"]["bottom"].as_f64().unwrap_or(f64::INFINITY) < body_top
                        && anchor_x >= body_left
                        && anchor_x <= body_left + 2.54
                } else {
                    field["bounds"]["top"].as_f64().unwrap_or(f64::NEG_INFINITY) > body_bottom
                        && anchor_x >= body_left
                        && anchor_x <= body_right
                };
                if !rotation_ok || !side_ok {
                    issues.push(json!({
                        "reference": instance.reference,
                        "field": if field_name == "reference_field" {"Reference"} else {"Value"},
                        "reason": "large_connector_field_layout",
                        "expected": {"rendered_rotation": 0, "side": expected_side},
                        "actual": field,
                        "rotation_ok": rotation_ok,
                        "side_ok": side_ok,
                        "body": body
                    }));
                }
            }
        }
        let Some(lib_sym) = resolve_embedded_pin_symbol(&lib_syms, &instance.lib_id) else {
            continue;
        };
        let Some(instance_node) = find_instance_node(&tree, &instance.reference) else {
            continue;
        };
        let body_bounds = SchBounds {
            left: body["left"].as_f64().unwrap_or(0.0),
            right: body["right"].as_f64().unwrap_or(0.0),
            top: body["top"].as_f64().unwrap_or(0.0),
            bottom: body["bottom"].as_f64().unwrap_or(0.0),
        };
        for (field_name, display_name) in [("Reference", "Reference"), ("Value", "Value")] {
            let Some((field_bounds, ..)) =
                property_bounds(instance_node, field_name, instance.rotation)
            else {
                continue;
            };
            for pin in extract_lib_pins(lib_sym) {
                let (pin_x, pin_y) = pin_endpoint(&pin, instance.pin_transform());
                let Some(corridor) = edge_pin_corridor(body_bounds, pin_x, pin_y, min_clearance)
                else {
                    continue;
                };
                let (_, intersects) = bounds_gap(field_bounds, corridor);
                if intersects {
                    issues.push(json!({
                        "reference": instance.reference,
                        "field": display_name,
                        "reason": "field_overlaps_pin_corridor",
                        "pin": {"number": pin.number, "name": pin.name, "x": pin_x, "y": pin_y},
                        "field_bounds": field_bounds.json(),
                        "corridor": corridor.json(),
                        "body": body
                    }));
                    break;
                }
            }
        }
    }

    Ok(CallToolResult::json(&json!({
        "checked_components": checked,
        "issue_count": issues.len(),
        "min_clearance": min_clearance,
        "max_clearance": max_clearance,
        "issues": issues
    })))
}

/// Resolve the embedded symbol that owns an instance's pin graphics.
///
/// KiCad aliases commonly contain only `(extends "Library:Parent")`; their
/// pins live on the embedded parent symbol.  Returning the alias itself makes
/// every pin-location query empty even though KiCad renders and connects the
/// instance correctly.
fn resolve_embedded_pin_symbol<'a>(
    lib_syms: &[&'a SexpNode],
    lib_id: &str,
) -> Option<&'a SexpNode> {
    let mut current = lib_id;
    for _ in 0..16 {
        let symbol = lib_syms
            .iter()
            .copied()
            .find(|node| node.get(1).and_then(|child| child.as_str()) == Some(current))?;
        if !extract_lib_pins(symbol).is_empty() {
            return Some(symbol);
        }
        match symbol
            .find("extends")
            .and_then(|node| node.get(1))
            .and_then(SexpNode::as_str)
        {
            Some(parent) => current = parent,
            None => return Some(symbol),
        }
    }
    None
}

async fn handle_get_schematic_view(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let tmp_dir = std::env::temp_dir().join(format!("konnect_{}", new_uuid()));
    tokio::fs::create_dir_all(&tmp_dir).await?;

    // KiCAD 10 CLI only supports SVG export for schematics (no bitmap)
    let svg_path =
        crate::tools::cli::render_schematic_svg(&ctx.config.kicad_cli, &sch_path, &tmp_dir).await?;

    let svg_content = tokio::fs::read_to_string(&svg_path).await?;
    tokio::fs::remove_dir_all(&tmp_dir).await.ok();

    // Return as text content (SVG is XML text, not a raster image)
    Ok(crate::mcp::protocol::CallToolResult {
        content: vec![crate::mcp::protocol::ToolContent::Text {
            text: format!("SVG schematic rendered. {} bytes.\n\nNote: KiCAD 10 CLI exports schematics as SVG only (no bitmap). \
                          The SVG file has been generated. Use export_schematic_pdf for a PDF version.", svg_content.len()),
        }],
        is_error: false,
    })
}

async fn handle_add_component_annotation(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };
    let key = match require_str(args, "key") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let value = match require_str(args, "value") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };

    let content = std::fs::read_to_string(&sch_path)?;

    // Find the symbol block for this reference
    let ref_search = format!(r#"(property "Reference" "{reference}""#);
    let ref_pos = match content.find(&ref_search) {
        Some(o) => o,
        None => {
            return Ok(CallToolResult::error(format!(
                "Component '{}' not found",
                reference
            )))
        }
    };

    let before = &content[..ref_pos];
    let sym_start = match ["\n  (symbol", "\n\t(symbol"]
        .iter()
        .filter_map(|pattern| before.rfind(pattern))
        .max()
    {
        Some(o) => o + 1,
        None => return Ok(CallToolResult::error("Could not find symbol block")),
    };
    let (_, sym_end) = match find_block_with_leading_whitespace(&content, sym_start) {
        Some(r) => r,
        None => return Ok(CallToolResult::error("Could not parse symbol block")),
    };

    // Find the position just before (instances in the symbol block, or before closing paren
    let sym_block = &content[sym_start..sym_end];
    let insert_rel = sym_block
        .find("(instances")
        .unwrap_or(sym_block.rfind(')').unwrap_or(sym_block.len() - 1));
    let insert_abs = sym_start + insert_rel;

    // Build the property S-expression
    let prop_sexp = format!(
        "    (property \"{key}\" \"{value}\"\n      (at 0 0 0)\n      (effects (font (size 1.27 1.27)) (hide yes))\n    )\n    "
    );

    let new_content = apply_edits(content, vec![SexpEdit::insert(insert_abs, prop_sexp)]);
    write_atomic(&sch_path, &new_content)?;

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "added_property": key,
        "value": value
    })))
}

async fn handle_group_components(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let group_name = match require_str(args, "group_name") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let refs = args["references"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if refs.is_empty() {
        return Ok(CallToolResult::error("No references provided"));
    }

    let mut content = std::fs::read_to_string(&sch_path)?;
    let mut grouped = Vec::new();

    for reference in &refs {
        let ref_search = format!(r#"(property "Reference" "{reference}""#);
        let ref_pos = match content.find(&ref_search) {
            Some(o) => o,
            None => continue,
        };

        let before = &content[..ref_pos];
        let sym_start = match before.rfind("\n  (symbol") {
            Some(o) => o + 1,
            None => continue,
        };
        let (_, sym_end) = match find_block_with_leading_whitespace(&content, sym_start) {
            Some(r) => r,
            None => continue,
        };

        let sym_block = &content[sym_start..sym_end];
        let insert_rel = sym_block
            .find("(instances")
            .unwrap_or(sym_block.rfind(')').unwrap_or(sym_block.len() - 1));
        let insert_abs = sym_start + insert_rel;

        let prop_sexp = format!(
            "    (property \"Group\" \"{group_name}\"\n      (at 0 0 0)\n      (effects (font (size 1.27 1.27)) (hide yes))\n    )\n    "
        );

        content = apply_edits(content, vec![SexpEdit::insert(insert_abs, prop_sexp)]);
        grouped.push(reference.clone());
    }

    write_atomic(&sch_path, &content)?;

    Ok(CallToolResult::json(&json!({
        "group_name": group_name,
        "grouped_count": grouped.len(),
        "grouped": grouped
    })))
}

async fn handle_replace_component(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(r) => r.to_string(),
        Err(e) => return Ok(e),
    };
    let new_lib_id = match require_str(args, "new_lib_id") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };

    let mut content = std::fs::read_to_string(&sch_path)?;

    // Find the symbol block for this reference
    let ref_search = format!(r#"(property "Reference" "{reference}""#);
    let ref_pos = match content.find(&ref_search) {
        Some(o) => o,
        None => {
            return Ok(CallToolResult::error(format!(
                "Component '{}' not found",
                reference
            )))
        }
    };

    let before = &content[..ref_pos];
    let sym_start = match ["\n  (symbol", "\n\t(symbol"]
        .iter()
        .filter_map(|pattern| before.rfind(pattern))
        .max()
    {
        Some(o) => o + 1,
        None => return Ok(CallToolResult::error("Could not find symbol block")),
    };

    // Find the (lib_id "OLD") and replace it
    let sym_block_start = &content[sym_start..];
    let lib_id_pat = "(lib_id \"";
    let lib_id_rel = match sym_block_start.find(lib_id_pat) {
        Some(o) => o,
        None => {
            return Ok(CallToolResult::error(
                "Could not find lib_id in symbol block",
            ))
        }
    };
    let lib_id_abs = sym_start + lib_id_rel + lib_id_pat.len();
    let lib_id_end = match content[lib_id_abs..].find('"') {
        Some(o) => lib_id_abs + o,
        None => return Ok(CallToolResult::error("Malformed lib_id")),
    };

    let old_lib_id = content[lib_id_abs..lib_id_end].to_string();

    let new_content = apply_edits(
        content,
        vec![SexpEdit::replace(
            lib_id_abs,
            lib_id_end,
            new_lib_id.clone(),
        )],
    );
    content = new_content;

    // Ensure the new library symbol definition is present
    super::ensure_lib_symbol_in_schematic(&mut content, &new_lib_id);

    // A verified project may contain a flattened or project-qualified embedded
    // definition that is more authoritative than the installed library alias.
    // Reuse that complete definition when explicitly requested.
    if let Some(source_path) = args["source_schematic"].as_str() {
        let source = std::fs::read_to_string(source_path)?;
        let source_def = super::extract_symbol_block(&source, &new_lib_id)
            .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found in source schematic", new_lib_id))?;
        let target_start = content.find(&format!("(symbol \"{}\"", new_lib_id))
            .ok_or_else(|| anyhow::anyhow!("Embedded symbol '{}' not found in target schematic", new_lib_id))?;
        let target_def = super::extract_symbol_block(&content[target_start..], &new_lib_id)
            .ok_or_else(|| anyhow::anyhow!("Could not parse embedded symbol '{}'", new_lib_id))?;
        content.replace_range(target_start..target_start + target_def.len(), &source_def);
    }
    write_atomic(&sch_path, &content)?;

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "old_lib_id": old_lib_id,
        "new_lib_id": new_lib_id
    })))
}

// Library symbol resolution moved to tools/mod.rs (shared with sch_wiring.rs)

#[cfg(test)]
mod symbol_bounds_tests {
    use super::{bounds_gap, edge_pin_corridor, rendered_field_rotation, SchBounds};

    #[test]
    fn field_rotation_is_resolved_in_symbol_frame() {
        assert_eq!(rendered_field_rotation(90.0, 0.0), 90.0);
        assert_eq!(rendered_field_rotation(90.0, 90.0), 0.0);
        assert_eq!(rendered_field_rotation(0.0, 0.0), 0.0);
    }

    #[test]
    fn bottom_pin_corridor_runs_from_body_edge_to_endpoint() {
        let body = SchBounds {
            left: 10.0,
            right: 30.0,
            top: 10.0,
            bottom: 20.0,
        };
        let corridor = edge_pin_corridor(body, 15.0, 25.0, 0.2).unwrap();
        assert_eq!(corridor.left, 14.8);
        assert_eq!(corridor.right, 15.2);
        assert_eq!(corridor.top, 20.0);
        assert_eq!(corridor.bottom, 25.0);
    }

    #[test]
    fn bounds_gap_reports_clearance_between_disjoint_boxes() {
        let body = SchBounds {
            left: 10.0,
            top: 10.0,
            right: 20.0,
            bottom: 20.0,
        };
        let field = SchBounds {
            left: 21.27,
            top: 12.0,
            right: 24.0,
            bottom: 14.0,
        };

        let (gap, overlaps) = bounds_gap(body, field);
        assert!(!overlaps);
        assert!((gap - 1.27).abs() < 1e-9);
    }

    #[test]
    fn bounds_gap_reports_overlapping_boxes() {
        let body = SchBounds {
            left: 10.0,
            top: 10.0,
            right: 20.0,
            bottom: 20.0,
        };
        let field = SchBounds {
            left: 18.0,
            top: 12.0,
            right: 24.0,
            bottom: 14.0,
        };

        let (gap, overlaps) = bounds_gap(body, field);
        assert!(overlaps);
        assert_eq!(gap, 0.0);
    }
}
