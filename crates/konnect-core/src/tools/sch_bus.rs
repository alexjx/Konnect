//! `sch_bus` toolset — buses, bus entries, and pin-to-bus fan-out.
//!
//! A KiCad bus is three things drawn together, and all three have to agree or
//! the bus carries nothing:
//!
//!   1. the **bus segment** itself — geometrically a wire, distinguished only
//!      by the node name and by the label attached to it;
//!   2. a **bus entry** per member — the 45° tick that bridges the gap between
//!      a wire and the bus. It is not decoration: without it the wire and the
//!      bus do not touch, because they are deliberately drawn apart;
//!   3. a **label on every wire stub**, naming the member signal, plus a label
//!      on the bus naming the group.
//!
//! Membership is by *name*, never by geometry: a stub joins the bus because its
//! label matches a member of the bus label, so `connect_pins_to_bus` writes all
//! three parts from one call rather than leaving the caller to line them up.
//!
//! Bus label syntax, both accepted by `add_schematic_net_label`:
//!   - vector  `X_DIG[1..6]`   — members `X_DIG1` … `X_DIG6`
//!   - group   `{A B C}`       — members named individually

use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{get_path, opt_f64, opt_str, require_f64, ToolContext, ToolDef};
use konnect_sexp::{
    parser::parse_sexp,
    schematic::{
        extract_symbol_instances, format_bus, format_bus_entry, format_net_label,
        horizontal_label_rotation, pin_endpoint, pin_outward_direction, BusEntryDirection,
    },
    writer::{write_atomic, write_atomic_if_unchanged},
};
use serde_json::json;

use super::sch_wiring::{
    direction_from_to, insert_wire_with_junctions, plan_connection_segments, resolve_placed_pin,
    ConnectionAnchor, ConnectionSegment,
};

pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "add_bus",
            "Add a bus segment between two points. Geometrically a wire, but KiCad treats it as a \
             bus: it carries the members named by the bus label attached to it (vector \
             'NAME[1..6]' or group '{A B C}'), and ordinary wires join it only through a bus entry.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "x1": { "type": "number" },
                    "y1": { "type": "number" },
                    "x2": { "type": "number" },
                    "y2": { "type": "number" }
                },
                "required": ["schematic", "x1", "y1", "x2", "y2"]
            }),
            |args, ctx| async move { handle_add_bus(args, ctx).await }
        ),
        tool!(
            "batch_add_bus",
            "Add multiple bus segments in a single file read/write cycle.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "buses": {
                        "type": "array",
                        "description": "List of {x1,y1,x2,y2} segments",
                        "items": {
                            "type": "object",
                            "properties": {
                                "x1": { "type": "number" }, "y1": { "type": "number" },
                                "x2": { "type": "number" }, "y2": { "type": "number" }
                            },
                            "required": ["x1", "y1", "x2", "y2"]
                        }
                    }
                },
                "required": ["schematic", "buses"]
            }),
            |args, ctx| async move { handle_batch_add_bus(args, ctx).await }
        ),
        tool!(
            "add_bus_entry",
            "Add a bus entry — the 45-degree tick that connects a wire to a bus. Required: a wire \
             and a bus that merely touch are NOT connected without one. 'x'/'y' are the end on \
             the wire side; 'direction' says which way the tick runs to reach the bus.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "x": { "type": "number", "description": "X of the wire-side end" },
                    "y": { "type": "number", "description": "Y of the wire-side end" },
                    "direction": {
                        "type": "string",
                        "description": "Which way the tick runs from 'at' to the bus",
                        "enum": ["down_right", "down_left", "up_right", "up_left"],
                        "default": "down_right"
                    }
                },
                "required": ["schematic", "x", "y"]
            }),
            |args, ctx| async move { handle_add_bus_entry(args, ctx).await }
        ),
        tool!(
            "connect_pins_to_bus",
            "Fan a set of pins out onto a bus: for each pin, a wire stub from the pin to the bus \
             entry, the bus entry itself, and a net label naming the member. This is the whole \
             connection — a stub without a label joins nothing, since bus membership is by name. \
             Each symbol pin first leaves outward by at least one 1.27 mm grid step. \
             Give the bus's fixed coordinate ('bus_y' for a horizontal bus, 'bus_x' for a \
             vertical one); the bus segment itself is drawn separately with add_bus.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "bus_y": { "type": "number", "description": "Y of a horizontal bus (omit if vertical)" },
                    "bus_x": { "type": "number", "description": "X of a vertical bus (omit if horizontal)" },
                    "connections": {
                        "type": "array",
                        "description": "List of {reference, pin_number, net} — net is the bus member name",
                        "items": {
                            "type": "object",
                            "properties": {
                                "reference": { "type": "string" },
                                "pin_number": { "type": "string" },
                                "net": { "type": "string" }
                            },
                            "required": ["reference", "pin_number", "net"]
                        }
                    },
                },
                "required": ["schematic", "connections"]
            }),
            |args, ctx| async move { handle_connect_pins_to_bus(args, ctx).await }
        ),
    ]
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn handle_add_bus(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let (x1, y1, x2, y2) = match read_xy_pair(args) {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    if (x1 - x2).abs() > 0.01 && (y1 - y2).abs() > 0.01 {
        return Ok(CallToolResult::error(
            "Bus segment must be horizontal or vertical",
        ));
    }

    let content = std::fs::read_to_string(&sch_path)?;
    let new_content =
        crate::tools::sch_wiring::insert_before_close(&content, &format_bus(x1, y1, x2, y2));
    write_atomic(&sch_path, &new_content)?;

    Ok(CallToolResult::json(&json!({
        "added": "bus",
        "from": { "x": x1, "y": y1 },
        "to": { "x": x2, "y": y2 }
    })))
}

async fn handle_batch_add_bus(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let items = match args["buses"].as_array() {
        Some(a) => a.clone(),
        None => return Ok(CallToolResult::error("Missing 'buses' array")),
    };

    let mut inserts = String::new();
    let mut added: Vec<serde_json::Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let (x1, y1, x2, y2) = match read_xy_pair(item) {
            Ok(v) => v,
            Err(_) => {
                errors.push(format!("Segment {i}: needs x1, y1, x2, y2"));
                continue;
            }
        };
        if (x1 - x2).abs() > 0.01 && (y1 - y2).abs() > 0.01 {
            errors.push(format!("Segment {i}: must be horizontal or vertical"));
            continue;
        }
        inserts.push('\n');
        inserts.push_str("  ");
        inserts.push_str(&format_bus(x1, y1, x2, y2));
        added.push(json!({ "from": {"x": x1, "y": y1}, "to": {"x": x2, "y": y2} }));
    }

    let content = std::fs::read_to_string(&sch_path)?;
    let new_content = crate::tools::sch_wiring::insert_before_close(&content, &inserts);
    write_atomic(&sch_path, &new_content)?;

    Ok(CallToolResult::json(&json!({
        "added_count": added.len(),
        "added": added,
        "errors": errors
    })))
}

async fn handle_add_bus_entry(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let x = match require_f64(args, "x") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let y = match require_f64(args, "y") {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let raw = opt_str(args, "direction").unwrap_or("down_right");
    let direction = match parse_direction(raw) {
        Some(d) => d,
        None => {
            return Ok(CallToolResult::error(format!(
                "Unknown direction '{raw}'. Valid: down_right, down_left, up_right, up_left"
            )))
        }
    };

    let content = std::fs::read_to_string(&sch_path)?;
    let new_content =
        crate::tools::sch_wiring::insert_before_close(&content, &format_bus_entry(x, y, direction));
    write_atomic(&sch_path, &new_content)?;

    // Which end is the bus side is a fact about the sheet, not about the
    // request: the entry's `at` corner is wherever the caller put it. The
    // documented convention (x/y = wire side) only holds when the caller
    // follows it, and the response used to assert it unchecked — labelling
    // the on-bus corner `wire_side` for anyone who gave the bus point
    // instead (#329). Judge both corners against the sheet's actual bus
    // segments and only fall back to the convention when geometry cannot
    // decide.
    let round6 = |value: f64| (value * 1e6).round() / 1e6;
    let far = (
        round6(x + direction.size().0),
        round6(y + direction.size().1),
    );
    let (at_on_bus, far_on_bus) = match parse_sexp(&content) {
        Ok(tree) => {
            let buses = konnect_sexp::schematic::extract_buses(&tree);
            let on_bus = |p: (f64, f64)| {
                buses.iter().any(|bus| {
                    konnect_sexp::geometry::point_on_segment(
                        p.0,
                        p.1,
                        bus.x1,
                        bus.y1,
                        bus.x2,
                        bus.y2,
                        crate::tools::sch_connectivity::COINCIDENT_TOLERANCE,
                    )
                })
            };
            (on_bus((x, y)), on_bus(far))
        }
        Err(_) => (false, false),
    };
    let (bus_side, wire_side, note) = match (at_on_bus, far_on_bus) {
        (true, false) => ((x, y), far, None),
        (false, true) => (far, (x, y), None),
        (true, true) => (
            far,
            (x, y),
            Some("both ends of this entry touch a bus; sides are reported from the documented convention (x/y = wire side)"),
        ),
        (false, false) => (
            far,
            (x, y),
            Some("no bus touches this entry; sides are reported from the documented convention (x/y = wire side)"),
        ),
    };
    let mut response = json!({
        "added": "bus_entry",
        "wire_side": { "x": wire_side.0, "y": wire_side.1 },
        "bus_side": { "x": bus_side.0, "y": bus_side.1 }
    });
    if let Some(note) = note {
        response["note"] = json!(note);
    }
    Ok(CallToolResult::json(&response))
}

async fn handle_connect_pins_to_bus(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let conns = match args["connections"].as_array() {
        Some(a) => a.clone(),
        None => return Ok(CallToolResult::error("Missing 'connections' array")),
    };
    let bus_y = opt_f64(args, "bus_y");
    let bus_x = opt_f64(args, "bus_x");
    let size = 2.54; // BusEntryDirection fixes the tick at one grid step

    let horizontal = match (bus_y, bus_x) {
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => {
            return Ok(CallToolResult::error(
                "Give exactly one of 'bus_y' (horizontal bus) or 'bus_x' (vertical bus)",
            ))
        }
    };

    let content = std::fs::read_to_string(&sch_path)?;
    let tree = match parse_sexp(&content) {
        Ok(t) => t,
        Err(e) => return Ok(CallToolResult::error(format!("Parse error: {e}"))),
    };
    let instances = extract_symbol_instances(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();

    let mut wire_segments = Vec::<ConnectionSegment>::new();
    let mut inserts = String::new();
    let mut added: Vec<serde_json::Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for conn in &conns {
        let (reference, pin_number, net) = match (
            conn["reference"].as_str(),
            conn["pin_number"].as_str(),
            conn["net"].as_str(),
        ) {
            (Some(r), Some(p), Some(n)) => (r, p, n),
            _ => {
                errors.push("Each connection needs reference, pin_number and net".into());
                continue;
            }
        };

        // Resolve against the unit that actually owns the pin — a multi-unit
        // part repeats its reference on every unit.
        let (pin, transform) =
            match resolve_placed_pin(&instances, &lib_syms, reference, pin_number) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
        let (px, py) = pin_endpoint(&pin, transform);
        let pin_anchor = ConnectionAnchor {
            x: px,
            y: py,
            outward: Some(pin_outward_direction(&pin, transform)),
        };

        // The entry bridges the last `size` before the bus; the stub covers the
        // rest of the way from the pin.
        let (entry_x, entry_y, dx, dy) = if horizontal {
            let by = bus_y.unwrap();
            let dir = if by > py { 1.0 } else { -1.0 };
            let ey = by - dir * size;
            (px, ey, dir * size, dir * size)
        } else {
            let bx = bus_x.unwrap();
            let dir = if bx > px { 1.0 } else { -1.0 };
            let ex = bx - dir * size;
            (ex, py, dir * size, dir * size)
        };

        let segments =
            match plan_connection_segments(pin_anchor, ConnectionAnchor::point(entry_x, entry_y)) {
                Ok(segments) => segments,
                Err(error) => {
                    errors.push(format!("{reference} pin {pin_number}: {error}"));
                    continue;
                }
            };
        let Some(last) = segments.last() else {
            errors.push(format!(
                "{reference} pin {pin_number}: connection planner returned no wire segment"
            ));
            continue;
        };
        let entry = (entry_x, entry_y);
        let other = if (last.x2 - entry_x).abs() < 0.01 && (last.y2 - entry_y).abs() < 0.01 {
            (last.x1, last.y1)
        } else if (last.x1 - entry_x).abs() < 0.01 && (last.y1 - entry_y).abs() < 0.01 {
            (last.x2, last.y2)
        } else {
            errors.push(format!(
                "{reference} pin {pin_number}: planned wire does not reach the bus entry"
            ));
            continue;
        };
        let label_rotation = match direction_from_to(other, entry) {
            Ok(direction) => horizontal_label_rotation(direction),
            Err(error) => {
                errors.push(format!("{reference} pin {pin_number}: {error}"));
                continue;
            }
        };
        wire_segments.extend(segments);
        inserts.push_str("\n  ");
        inserts.push_str(&format_bus_entry(entry_x, entry_y, entry_dir(dx, dy)));
        // The label is what actually puts this stub on the bus. Anchor it at
        // the wire side of the entry and face it from the final wire segment;
        // putting it back on the pin bypasses the visible outgoing stub.
        inserts.push_str("\n  ");
        inserts.push_str(&format_net_label(net, entry_x, entry_y, label_rotation));

        added.push(json!({
            "reference": reference, "pin": pin_number, "net": net,
            "pin_at": { "x": px, "y": py },
            "entry_at": { "x": entry_x, "y": entry_y },
            "label": { "x": entry_x, "y": entry_y, "rotation": label_rotation }
        }));
    }

    let mut new_content = content.clone();
    for segment in wire_segments {
        new_content =
            insert_wire_with_junctions(new_content, segment.x1, segment.y1, segment.x2, segment.y2);
    }
    let new_content = crate::tools::sch_wiring::insert_before_close(&new_content, &inserts);
    write_atomic_if_unchanged(&sch_path, &content, &new_content)?;

    Ok(CallToolResult::json(&json!({
        "connected_count": added.len(),
        "connected": added,
        "errors": errors
    })))
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Map a signed (dx, dy) tick offset onto KiCad's four bus-entry directions.
fn entry_dir(dx: f64, dy: f64) -> BusEntryDirection {
    match (dx > 0.0, dy > 0.0) {
        (true, true) => BusEntryDirection::DownRight,
        (false, true) => BusEntryDirection::DownLeft,
        (true, false) => BusEntryDirection::UpRight,
        (false, false) => BusEntryDirection::UpLeft,
    }
}

fn parse_direction(s: &str) -> Option<BusEntryDirection> {
    Some(match s {
        "down_right" => BusEntryDirection::DownRight,
        "down_left" => BusEntryDirection::DownLeft,
        "up_right" => BusEntryDirection::UpRight,
        "up_left" => BusEntryDirection::UpLeft,
        _ => return None,
    })
}

fn read_xy_pair(v: &serde_json::Value) -> Result<(f64, f64, f64, f64), CallToolResult> {
    Ok((
        require_f64(v, "x1")?,
        require_f64(v, "y1")?,
        require_f64(v, "x2")?,
        require_f64(v, "y2")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet_with_bus() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bus.kicad_sch");
        let mut content = String::from(
            "(kicad_sch\n  (version 20260306)\n  (uuid \"11111111-1111-4111-8111-111111111111\")\n",
        );
        content.push_str(&format_bus(100.33, 100.33, 150.11, 100.33));
        content.push_str("\n)\n");
        std::fs::write(&path, content).unwrap();
        (directory, path)
    }

    fn sheet_with_symbol_and_bus() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("symbol-bus.kicad_sch");
        std::fs::write(
            &path,
            "(kicad_sch
  (version 20260306)
  (uuid \"11111111-1111-4111-8111-111111111111\")
  (lib_symbols
    (symbol \"Test:P\"
      (symbol \"P_1_1\"
        (pin passive line (at 0 0 0) (length 2.54)
          (name \"~\") (number \"1\")))))
  (bus (pts (xy 80 120) (xy 120 120)) (stroke (width 0) (type default))
    (uuid \"22222222-2222-4222-8222-222222222222\"))
  (wire (pts (xy 98.73 110) (xy 110 110)) (stroke (width 0) (type default))
    (uuid \"44444444-4444-4444-8444-444444444444\"))
  (symbol
    (lib_id \"Test:P\")
    (at 100 100 0)
    (unit 1)
    (uuid \"33333333-3333-4333-8333-333333333333\")
    (property \"Reference\" \"U1\" (at 100 95 0))))
",
        )
        .unwrap();
        (directory, path)
    }

    async fn entry_response(path: &std::path::Path, x: f64, y: f64) -> serde_json::Value {
        let result = handle_add_bus_entry(
            &json!({ "schematic": path, "x": x, "y": y, "direction": "down_right" }),
            &crate::tools::ToolContext::new(
                crate::tools::ServerConfig::default(),
                std::sync::Arc::new(crate::router::ToolRouter::new()),
            ),
        )
        .await
        .unwrap();
        assert!(!result.is_error, "add_bus_entry failed");
        let crate::mcp::protocol::ToolContent::Text { text } = &result.content[0] else {
            panic!("expected text");
        };
        serde_json::from_str(text).unwrap()
    }

    /// #329: which end touches the bus is a fact about the sheet. The caller
    /// here places the entry's `at` ON the bus (contrary to the documented
    /// x/y-is-the-wire-side convention, but exactly how people use it), and
    /// the response must label the corners from the geometry, not the
    /// convention — the old response called the on-bus corner `wire_side`,
    /// and a caller following it attached the wire to the bus itself, where
    /// it connects nothing.
    #[tokio::test]
    async fn bus_side_is_the_corner_that_actually_touches_the_bus() {
        let (_directory, path) = sheet_with_bus();
        let response = entry_response(&path, 120.65, 100.33).await;
        assert_eq!(response["bus_side"]["x"], 120.65, "{response}");
        assert_eq!(response["bus_side"]["y"], 100.33);
        assert_eq!(response["wire_side"]["x"], 123.19);
        assert_eq!(response["wire_side"]["y"], 102.87);
        assert!(
            response.get("note").is_none(),
            "geometry decided: {response}"
        );
    }

    /// The documented usage — x/y at the wire end, the tick running to the
    /// bus — keeps its labels, now confirmed by geometry rather than assumed.
    #[tokio::test]
    async fn the_documented_wire_side_placement_keeps_its_labels() {
        let (_directory, path) = sheet_with_bus();
        let response = entry_response(&path, 120.65, 97.79).await;
        assert_eq!(response["wire_side"]["x"], 120.65, "{response}");
        assert_eq!(response["wire_side"]["y"], 97.79);
        assert_eq!(response["bus_side"]["x"], 123.19);
        assert_eq!(response["bus_side"]["y"], 100.33);
        assert!(response.get("note").is_none(), "{response}");
    }

    /// With no bus near the entry, geometry cannot decide — the response
    /// falls back to the documented convention and says so instead of
    /// silently asserting.
    #[tokio::test]
    async fn a_floating_entry_reports_the_convention_and_says_so() {
        let (_directory, path) = sheet_with_bus();
        let response = entry_response(&path, 30.0, 30.0).await;
        assert_eq!(response["wire_side"]["x"], 30.0, "{response}");
        assert_eq!(response["bus_side"]["x"], 32.54);
        let note = response["note"].as_str().expect("note present");
        assert!(note.contains("no bus touches"), "{note}");
    }

    #[tokio::test]
    async fn pin_to_bus_connection_leaves_the_symbol_outward_before_turning() {
        let (_directory, path) = sheet_with_symbol_and_bus();
        let result = handle_connect_pins_to_bus(
            &json!({
                "schematic": path.display().to_string(),
                "bus_y": 120.0,
                "connections": [{ "reference": "U1", "pin_number": "1", "net": "DATA" }]
            }),
            &crate::tools::ToolContext::new(
                crate::tools::ServerConfig::default(),
                std::sync::Arc::new(crate::router::ToolRouter::new()),
            ),
        )
        .await
        .unwrap();

        assert!(!result.is_error, "{result:?}");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("(xy 100 100) (xy 98.73 100)"), "{after}");
        assert!(after.contains("(bus_entry"), "{after}");
        let tree = konnect_sexp::parse_sexp(&after).unwrap();
        let label = konnect_sexp::schematic::extract_labels(&tree)
            .into_iter()
            .find(|label| label.net == "DATA")
            .expect("member label");
        assert_eq!((label.x, label.y, label.rotation), (100.0, 117.46, 0.0));
        assert!(
            konnect_sexp::schematic::extract_junctions(&tree)
                .iter()
                .any(|&(x, y)| (x - 98.73).abs() < 0.01 && (y - 110.0).abs() < 0.01),
            "the new route T-lands on an existing wire: {after}"
        );
    }

    #[test]
    fn bus_sexp_is_a_bus_node_not_a_wire() {
        let s = format_bus(10.0, 20.0, 50.0, 20.0);
        assert!(s.starts_with("(bus"), "got {s}");
        assert!(s.contains("(xy 10 20) (xy 50 20)"));
    }

    /// `size` is the offset to the bus side, so the two ends are `at` and
    /// `at + size`. Getting the sign wrong puts the tick on the far side of the
    /// bus, where it connects nothing.
    #[test]
    fn entry_direction_follows_the_offset_signs() {
        assert!(matches!(
            entry_dir(2.54, 2.54),
            BusEntryDirection::DownRight
        ));
        assert!(matches!(
            entry_dir(-2.54, 2.54),
            BusEntryDirection::DownLeft
        ));
        assert!(matches!(entry_dir(2.54, -2.54), BusEntryDirection::UpRight));
        assert!(matches!(entry_dir(-2.54, -2.54), BusEntryDirection::UpLeft));
    }

    #[test]
    fn direction_names_round_trip() {
        for (name, dir) in [
            ("down_right", BusEntryDirection::DownRight),
            ("down_left", BusEntryDirection::DownLeft),
            ("up_right", BusEntryDirection::UpRight),
            ("up_left", BusEntryDirection::UpLeft),
        ] {
            assert_eq!(parse_direction(name).unwrap().size(), dir.size());
        }
        assert!(parse_direction("sideways").is_none());
    }

    #[test]
    fn all_four_tools_are_registered() {
        let names: Vec<_> = tools().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "add_bus",
                "batch_add_bus",
                "add_bus_entry",
                "connect_pins_to_bus"
            ]
        );
    }
}
