//! `sch_analysis` toolset — net connectivity, pin queries, trace paths, overlap/orphan detection.
//!
//! All operations are read-only S-expression analysis.
//! Net graph uses union-find (O(W+L+P)), matching net_analysis.py.

use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{get_path, opt_f64, require_f64, require_str, ToolContext, ToolDef};
use konnect_schematic_editor as cse;
use konnect_sexp::{
    geometry::{point_on_segment, points_coincident},
    schematic::{
        extract_labels, extract_lib_pins, extract_symbol_instances, extract_wires, pin_endpoint,
        read_schematic, Wire,
    },
    SexpNode,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "list_schematic_wires",
            "List all wire segments in a schematic with start/end coordinates and UUIDs.",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_list_wires(args, ctx).await }
        ),
        tool!(
            "list_schematic_nets",
            "List all distinct net names derived from net labels, global labels, and power symbols.",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_list_nets(args, ctx).await }
        ),
        tool!(
            "list_schematic_labels",
            "List all label instances (net_label, global_label, hierarchical_label) \
             with their positions, net names, and types.",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_list_labels(args, ctx).await }
        ),
        tool!(
            "get_net_connections",
            "Get all pins and labels connected to a named net.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "net": { "type": "string", "description": "Net name to query" }
                },
                "required": ["schematic", "net"] }),
            |args, ctx| async move { handle_get_net_connections(args, ctx).await }
        ),
        tool!(
            "get_net_connectivity",
            "Build the full connectivity graph for a net using union-find. \
             Returns all wire segments, labels, and T-junction locations.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "net": { "type": "string" }
                },
                "required": ["schematic", "net"] }),
            |args, ctx| async move { handle_get_net_connectivity(args, ctx).await }
        ),
        tool!(
            "get_pin_connections",
            "Get the net connected to a specific pin on a component by tracing wires from the pin endpoint.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" },
                    "pin_number": { "type": "string" }
                },
                "required": ["schematic", "reference", "pin_number"] }),
            |args, ctx| async move { handle_get_pin_connections(args, ctx).await }
        ),
        tool!(
            "get_pin_net_name",
            "Return just the net name for a specific pin on a component.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" },
                    "pin_number": { "type": "string" }
                },
                "required": ["schematic", "reference", "pin_number"] }),
            |args, ctx| async move { handle_get_pin_connections(args, ctx).await }
        ),
        tool!(
            "get_component_nets",
            "Get all nets connected to every pin of a component.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string" }
                },
                "required": ["schematic", "reference"] }),
            |args, ctx| async move { handle_get_component_nets(args, ctx).await }
        ),
        tool!(
            "get_net_components",
            "Get all components (and their pins) connected to a named net.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "net": { "type": "string" }
                },
                "required": ["schematic", "net"] }),
            |args, ctx| async move { handle_get_net_components(args, ctx).await }
        ),
        tool!(
            "trace_from_point",
            "Trace connectivity from any (X,Y) point — returns what is at that point and the net it belongs to.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "x": { "type": "number" }, "y": { "type": "number" },
                    "tolerance": { "type": "number", "default": 0.05 }
                },
                "required": ["schematic", "x", "y"] }),
            |args, ctx| async move { handle_trace_from_point(args, ctx).await }
        ),
        tool!(
            "find_orphan_items",
            "Find dangling wire ends, floating labels, and unconnected pin endpoints (0.05mm tolerance).",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_find_orphan_items(args, ctx).await }
        ),
        tool!(
            "find_shorted_nets",
            "Detect accidentally merged nets — pairs of distinct net names sharing a wire path.",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_find_shorted_nets(args, ctx).await }
        ),
        tool!(
            "find_single_pin_nets",
            "Find nets with only one label/connection — often indicates a missing counterpart.",
            json!({ "type": "object",
                "properties": { "schematic": { "type": "string" } },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_find_single_pin_nets(args, ctx).await }
        ),
        tool!(
            "get_connected_items",
            "Get all wires, labels, and components connected to a given component reference \
             by tracing net connectivity from each of its pins.",
            json!({
                "type": "object",
                "properties": {
                    "schematic": { "type": "string", "description": "Path to .kicad_sch file" },
                    "reference": { "type": "string", "description": "Component reference designator (e.g. 'R1')" }
                },
                "required": ["schematic", "reference"]
            }),
            |args, ctx| async move { handle_get_connected_items(args, ctx).await }
        ),
        tool!(
            "check_schematic_overlaps",
            "Find overlapping symbols or labels that may indicate placement errors.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "tolerance": { "type": "number", "default": 0.5 }
                },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_check_overlaps(args, ctx).await }
        ),
        tool!(
            "get_schematic_connection_islands",
            "Return conservative axis-aligned bounds for each physically connected schematic island. \
             An island joins symbols through their pins and drawn wires; labels terminate an island \
             but equal label names elsewhere do not merge islands. Bounds include symbol bodies, \
             pin endpoints, Reference/Value fields, wires, and estimated rendered label extents.",
            json!({ "type": "object",
                "properties": {
                    "schematic": { "type": "string" },
                    "reference": { "type": "string", "description": "Optional component reference; return only its island" },
                    "clearance": { "type": "number", "default": 2.54, "description": "Required AABB clearance in mm when reporting island conflicts" },
                    "connect_tolerance": { "type": "number", "default": 0.05, "description": "Coordinate tolerance in mm for physical connections" }
                },
                "required": ["schematic"] }),
            |args, ctx| async move { handle_get_connection_islands(args, ctx).await }
        ),
    ]
}

// ─── Union-Find net graph ─────────────────────────────────────────────────────

pub(crate) fn pt_key(x: f64, y: f64) -> (i64, i64) {
    ((x * 1000.0).round() as i64, (y * 1000.0).round() as i64)
}

pub(crate) struct NetGraph {
    pub(crate) point_nets: HashMap<(i64, i64), String>,
    pub(crate) parent: HashMap<(i64, i64), (i64, i64)>,
}

impl NetGraph {
    pub(crate) fn new() -> Self {
        NetGraph {
            point_nets: HashMap::new(),
            parent: HashMap::new(),
        }
    }

    pub(crate) fn ensure(&mut self, k: (i64, i64)) {
        self.parent.entry(k).or_insert(k);
    }

    pub(crate) fn find(&mut self, k: (i64, i64)) -> (i64, i64) {
        self.ensure(k);
        let p = self.parent[&k];
        if p == k {
            return k;
        }
        let root = self.find(p);
        self.parent.insert(k, root);
        root
    }

    pub(crate) fn union(&mut self, a: (i64, i64), b: (i64, i64)) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent.insert(rb, ra);
        }
    }

    pub(crate) fn add_wire(&mut self, w: &Wire) {
        let a = pt_key(w.x1, w.y1);
        let b = pt_key(w.x2, w.y2);
        self.ensure(a);
        self.ensure(b);
        self.union(a, b);
    }

    pub(crate) fn add_label(&mut self, x: f64, y: f64, net: &str) {
        let k = pt_key(x, y);
        self.ensure(k);
        self.point_nets.insert(k, net.to_string());
    }

    pub(crate) fn net_at(&mut self, x: f64, y: f64) -> Option<String> {
        let k = pt_key(x, y);
        self.ensure(k);
        let root = self.find(k);
        let labels: Vec<_> = self.point_nets.clone().into_iter().collect();
        for (lk, net) in labels {
            if self.find(lk) == root {
                return Some(net);
            }
        }
        None
    }

    pub(crate) fn points_on_net(&mut self, net: &str) -> Vec<(i64, i64)> {
        // Collect keys first to avoid simultaneous borrow of point_nets and self.find()
        let net_keys: Vec<(i64, i64)> = self
            .point_nets
            .iter()
            .filter(|(_, n)| n.as_str() == net)
            .map(|(k, _)| *k)
            .collect();
        let net_roots: HashSet<(i64, i64)> = net_keys.iter().map(|k| self.find(*k)).collect();
        let all_keys: Vec<(i64, i64)> = self.parent.keys().cloned().collect();
        all_keys
            .into_iter()
            .filter(|k| net_roots.contains(&self.find(*k)))
            .collect()
    }
}

pub(crate) fn build_net_graph(
    wires: &[Wire],
    labels: &[konnect_sexp::schematic::Label],
) -> NetGraph {
    let mut g = NetGraph::new();
    for w in wires {
        g.add_wire(w);
    }
    for l in labels {
        g.add_label(l.x, l.y, &l.net);
    }
    g
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn handle_list_wires(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let items: Vec<serde_json::Value> = sch.wires.iter()
        .map(|w| json!({ "x1": w.start.0, "y1": w.start.1, "x2": w.end.0, "y2": w.end.1, "uuid": w.uuid }))
        .collect();
    Ok(CallToolResult::json(
        &json!({ "count": items.len(), "wires": items }),
    ))
}

async fn handle_list_nets(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let mut nets: Vec<String> = sch
        .labels
        .iter()
        .map(|l| l.text.clone())
        .chain(sch.global_labels.iter().map(|l| l.text.clone()))
        .chain(sch.hierarchical_labels.iter().map(|l| l.text.clone()))
        .collect();
    nets.sort();
    nets.dedup();
    Ok(CallToolResult::json(
        &json!({ "count": nets.len(), "nets": nets }),
    ))
}

async fn handle_list_labels(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let mut items: Vec<serde_json::Value> = Vec::new();
    for l in sch.labels.iter() {
        items.push(json!({ "net": l.text, "type": "NetLabel", "x": l.at.x, "y": l.at.y, "rotation": l.at.rotation.unwrap_or(0.0), "uuid": l.uuid }));
    }
    for g in sch.global_labels.iter() {
        items.push(json!({ "net": g.text, "type": "GlobalLabel", "x": g.at.x, "y": g.at.y, "rotation": g.at.rotation.unwrap_or(0.0), "uuid": g.uuid }));
    }
    for h in sch.hierarchical_labels.iter() {
        items.push(json!({ "net": h.text, "type": "HierarchicalLabel", "x": h.at.x, "y": h.at.y, "rotation": h.at.rotation.unwrap_or(0.0), "uuid": h.uuid }));
    }
    Ok(CallToolResult::json(
        &json!({ "count": items.len(), "labels": items }),
    ))
}

async fn handle_get_net_connections(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let net = match require_str(args, "net") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let sch = cse::Schematic::load(&sch_path)?;
    let wires = super::sch_bridge::all_wires_as_sexp(&sch);
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let matching: Vec<_> = labels
        .iter()
        .filter(|l| l.net == net)
        .map(|l| json!({ "type": format!("{:?}", l.kind), "x": l.x, "y": l.y }))
        .collect();
    let mut g = build_net_graph(&wires, &labels);
    let pts = g.points_on_net(&net).len();
    Ok(CallToolResult::json(
        &json!({ "net": net, "label_count": matching.len(), "labels": matching, "connected_points": pts }),
    ))
}

async fn handle_get_net_connectivity(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let net = match require_str(args, "net") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let sch = cse::Schematic::load(&sch_path)?;
    let wires = super::sch_bridge::all_wires_as_sexp(&sch);
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let mut g = build_net_graph(&wires, &labels);
    let net_pts: HashSet<(i64, i64)> = g.points_on_net(&net).into_iter().collect();
    let net_wires: Vec<_> = wires
        .iter()
        .filter(|w| net_pts.contains(&pt_key(w.x1, w.y1)) || net_pts.contains(&pt_key(w.x2, w.y2)))
        .map(|w| json!({ "x1": w.x1, "y1": w.y1, "x2": w.x2, "y2": w.y2 }))
        .collect();
    let net_labels: Vec<_> = labels
        .iter()
        .filter(|l| l.net == net)
        .map(|l| json!({ "type": format!("{:?}", l.kind), "x": l.x, "y": l.y }))
        .collect();
    let net_wire_objs: Vec<Wire> = wires
        .iter()
        .filter(|w| net_pts.contains(&pt_key(w.x1, w.y1)) || net_pts.contains(&pt_key(w.x2, w.y2)))
        .cloned()
        .collect();
    let t_junctions = konnect_sexp::schematic::find_t_junctions(&net_wire_objs, 0.01);
    Ok(CallToolResult::json(&json!({
        "net": net,
        "wires": net_wires,
        "labels": net_labels,
        "t_junctions": t_junctions.iter().map(|(x,y)| json!({"x": x, "y": y})).collect::<Vec<_>>()
    })))
}

async fn handle_get_pin_connections(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let pin_number = match require_str(args, "pin_number") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let wires = extract_wires(&tree);
    let labels = extract_labels(&tree);
    let inst = instances
        .iter()
        .find(|i| i.reference == reference)
        .ok_or_else(|| anyhow::anyhow!("Component '{}' not found", reference))?;
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();
    let lib_sym = super::resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id);
    let pin_ep = lib_sym.and_then(|sym| {
        konnect_sexp::schematic::extract_lib_pins(sym)
            .iter()
            .find(|p| p.number == pin_number)
            .map(|p| konnect_sexp::schematic::pin_endpoint(p, inst.pin_transform()))
    });
    let (px, py) = match pin_ep {
        Some(ep) => ep,
        None => {
            return Ok(CallToolResult::error(format!(
                "Pin '{}' not found on '{}'",
                pin_number, reference
            )))
        }
    };
    let mut g = build_net_graph(&wires, &labels);
    Ok(CallToolResult::json(
        &json!({ "reference": reference, "pin": pin_number, "pin_x": px, "pin_y": py, "net": g.net_at(px, py) }),
    ))
}

async fn handle_get_component_nets(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let wires = extract_wires(&tree);
    let labels = extract_labels(&tree);
    let inst = instances
        .iter()
        .find(|i| i.reference == reference)
        .ok_or_else(|| anyhow::anyhow!("Component '{}' not found", reference))?;
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();
    let lib_sym = super::resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id);
    let mut g = build_net_graph(&wires, &labels);
    let pins: Vec<serde_json::Value> = if let Some(sym) = lib_sym {
        let t = inst.pin_transform();
        konnect_sexp::schematic::extract_lib_pins(sym).iter().map(|p| {
            let (px, py) = konnect_sexp::schematic::pin_endpoint(p, t);
            json!({ "pin": p.number, "name": p.name, "x": px, "y": py, "net": g.net_at(px, py) })
        }).collect()
    } else {
        Vec::new()
    };
    Ok(CallToolResult::json(
        &json!({ "reference": reference, "pins": pins }),
    ))
}

async fn handle_get_net_components(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let net = match require_str(args, "net") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let wires = extract_wires(&tree);
    let labels = extract_labels(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();
    let mut g = build_net_graph(&wires, &labels);
    let net_pts: HashSet<(i64, i64)> = g.points_on_net(&net).into_iter().collect();
    let result: Vec<serde_json::Value> = instances
        .iter()
        .filter_map(|inst| {
            let ls = super::resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id)?;
            let t = inst.pin_transform();
            let connected: Vec<_> = konnect_sexp::schematic::extract_lib_pins(ls)
                .iter()
                .filter_map(|p| {
                    let (px, py) = konnect_sexp::schematic::pin_endpoint(p, t);
                    if net_pts.contains(&pt_key(px, py)) {
                        Some(json!({ "pin": p.number, "name": p.name }))
                    } else {
                        None
                    }
                })
                .collect();
            if connected.is_empty() {
                None
            } else {
                Some(json!({ "reference": inst.reference, "value": inst.value, "pins": connected }))
            }
        })
        .collect();
    Ok(CallToolResult::json(
        &json!({ "net": net, "components": result }),
    ))
}

async fn handle_trace_from_point(
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
    let tol = opt_f64(args, "tolerance").unwrap_or(0.05);
    let sch = cse::Schematic::load(&sch_path)?;
    let wires = super::sch_bridge::all_wires_as_sexp(&sch);
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let mut g = build_net_graph(&wires, &labels);
    let on_wire: Vec<_> = wires
        .iter()
        .filter(|w| {
            points_coincident(x, y, w.x1, w.y1, tol)
                || points_coincident(x, y, w.x2, w.y2, tol)
                || point_on_segment(x, y, w.x1, w.y1, w.x2, w.y2, tol)
        })
        .map(|w| json!({ "x1": w.x1, "y1": w.y1, "x2": w.x2, "y2": w.y2 }))
        .collect();
    let at_label: Vec<_> = labels
        .iter()
        .filter(|l| points_coincident(x, y, l.x, l.y, tol))
        .map(|l| json!({ "net": l.net, "type": format!("{:?}", l.kind) }))
        .collect();
    Ok(CallToolResult::json(
        &json!({ "x": x, "y": y, "net": g.net_at(x, y), "wires_here": on_wire, "labels_here": at_label }),
    ))
}

async fn handle_find_orphan_items(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let (_, tree) = read_schematic(&sch_path)?;
    let wires = super::sch_bridge::all_wires_as_sexp(&sch);
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let label_pts: HashSet<(i64, i64)> = labels.iter().map(|l| pt_key(l.x, l.y)).collect();
    let instances = extract_symbol_instances(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|node| node.find_all("symbol"))
        .unwrap_or_default();
    let mut pin_pts: HashSet<(i64, i64)> = HashSet::new();
    for inst in &instances {
        if let Some(lib_sym) = super::resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id) {
            for pin in extract_lib_pins(lib_sym) {
                let (x, y) = pin_endpoint(&pin, inst.pin_transform());
                pin_pts.insert(pt_key(x, y));
            }
        }
    }
    let mut endpoint_counts: HashMap<(i64, i64), usize> = HashMap::new();
    for w in &wires {
        *endpoint_counts.entry(pt_key(w.x1, w.y1)).or_insert(0) += 1;
        *endpoint_counts.entry(pt_key(w.x2, w.y2)).or_insert(0) += 1;
    }
    let dangling: Vec<serde_json::Value> = endpoint_counts.iter()
        .filter(|(k, &c)| c == 1 && !label_pts.contains(k) && !pin_pts.contains(k))
        .map(|(k, _)| json!({ "type": "dangling_wire_end", "x": k.0 as f64/1000.0, "y": k.1 as f64/1000.0 }))
        .collect();
    let floating: Vec<serde_json::Value> = labels
        .iter()
        .filter(|l| !endpoint_counts.contains_key(&pt_key(l.x, l.y)))
        .map(|l| json!({ "type": "floating_label", "net": l.net, "x": l.x, "y": l.y }))
        .collect();
    let mut all = dangling;
    all.extend(floating);
    Ok(CallToolResult::json(
        &json!({ "orphan_count": all.len(), "orphans": all }),
    ))
}

async fn handle_find_shorted_nets(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let wires = super::sch_bridge::all_wires_as_sexp(&sch);
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let mut g = build_net_graph(&wires, &labels);
    let mut root_nets: HashMap<(i64, i64), Vec<String>> = HashMap::new();
    for l in &labels {
        let root = g.find(pt_key(l.x, l.y));
        root_nets.entry(root).or_default().push(l.net.clone());
    }
    let shorts: Vec<serde_json::Value> = root_nets
        .into_values()
        .filter_map(|mut nets| {
            nets.sort();
            nets.dedup();
            if nets.len() > 1 {
                Some(json!({ "shorted_nets": nets }))
            } else {
                None
            }
        })
        .collect();
    Ok(CallToolResult::json(
        &json!({ "short_count": shorts.len(), "shorts": shorts }),
    ))
}

async fn handle_find_single_pin_nets(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let sch = cse::Schematic::load(&sch_path)?;
    let labels = super::sch_bridge::all_labels_as_sexp(&sch);
    let mut counts: HashMap<String, usize> = HashMap::new();
    for l in &labels {
        *counts.entry(l.net.clone()).or_insert(0) += 1;
    }
    let singles: Vec<serde_json::Value> = counts
        .iter()
        .filter(|(_, &c)| c == 1)
        .map(|(net, _)| {
            let l = labels.iter().find(|l| &l.net == net).unwrap();
            json!({ "net": net, "x": l.x, "y": l.y, "type": format!("{:?}", l.kind) })
        })
        .collect();
    Ok(CallToolResult::json(
        &json!({ "single_pin_net_count": singles.len(), "nets": singles }),
    ))
}

async fn handle_get_connected_items(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference = match require_str(args, "reference") {
        Ok(v) => v.to_string(),
        Err(e) => return Ok(e),
    };

    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let wires = extract_wires(&tree);
    let labels = extract_labels(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|n| n.find_all("symbol"))
        .unwrap_or_default();

    let inst = match instances.iter().find(|i| i.reference == reference) {
        Some(i) => i,
        None => {
            return Ok(CallToolResult::error(format!(
                "Component '{}' not found",
                reference
            )))
        }
    };

    let lib_sym = super::resolve_embedded_pin_symbol(&lib_syms, &inst.lib_id);
    let mut g = build_net_graph(&wires, &labels);

    // Get nets for each pin
    let mut connected_nets: HashSet<String> = HashSet::new();
    if let Some(sym) = lib_sym {
        let t = inst.pin_transform();
        for p in konnect_sexp::schematic::extract_lib_pins(sym) {
            let (px, py) = konnect_sexp::schematic::pin_endpoint(&p, t);
            if let Some(net) = g.net_at(px, py) {
                connected_nets.insert(net);
            }
        }
    }

    // Find all wires, labels, and components on those nets
    let mut all_net_pts: HashSet<(i64, i64)> = HashSet::new();
    for net in &connected_nets {
        for pt in g.points_on_net(net) {
            all_net_pts.insert(pt);
        }
    }

    let connected_wires: Vec<serde_json::Value> = wires
        .iter()
        .filter(|w| {
            all_net_pts.contains(&pt_key(w.x1, w.y1)) || all_net_pts.contains(&pt_key(w.x2, w.y2))
        })
        .map(|w| json!({ "x1": w.x1, "y1": w.y1, "x2": w.x2, "y2": w.y2, "uuid": w.uuid }))
        .collect();

    let connected_labels: Vec<serde_json::Value> = labels
        .iter()
        .filter(|l| connected_nets.contains(&l.net))
        .map(|l| json!({ "net": l.net, "type": format!("{:?}", l.kind), "x": l.x, "y": l.y }))
        .collect();

    // Find other components on the same nets (excluding the queried one)
    let connected_components: Vec<serde_json::Value> = instances.iter()
        .filter(|i| i.reference != reference)
        .filter_map(|i| {
            let ls = super::resolve_embedded_pin_symbol(&lib_syms, &i.lib_id)?;
            let t = i.pin_transform();
            let matching_pins: Vec<_> = konnect_sexp::schematic::extract_lib_pins(ls).iter()
                .filter_map(|p| {
                    let (px, py) = konnect_sexp::schematic::pin_endpoint(p, t);
                    if all_net_pts.contains(&pt_key(px, py)) {
                        Some(json!({ "pin": p.number, "name": p.name }))
                    } else { None }
                }).collect();
            if matching_pins.is_empty() { None }
            else { Some(json!({ "reference": i.reference, "value": i.value, "connected_pins": matching_pins })) }
        })
        .collect();

    Ok(CallToolResult::json(&json!({
        "reference": reference,
        "nets": connected_nets.iter().collect::<Vec<_>>(),
        "connected_wires": connected_wires.len(),
        "wires": connected_wires,
        "labels": connected_labels,
        "connected_components": connected_components
    })))
}

async fn handle_check_overlaps(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let tol = opt_f64(args, "tolerance").unwrap_or(0.5);
    let sch = cse::Schematic::load(&sch_path)?;

    // Component overlap detection using the new crate's spatial query
    let symbols: Vec<&cse::Symbol> = sch.symbols.iter().collect();
    let mut comp_overlaps: Vec<serde_json::Value> = Vec::new();
    for (i, a) in symbols.iter().enumerate() {
        let (ax, ay) = a.position();
        for b in &symbols[i + 1..] {
            let (bx, by) = b.position();
            if points_coincident(ax, ay, bx, by, tol) {
                comp_overlaps.push(json!({
                    "type": "component_overlap",
                    "a": a.reference().unwrap_or("?"),
                    "b": b.reference().unwrap_or("?"),
                    "x": ax, "y": ay
                }));
            }
        }
    }

    // Label overlap detection — collect all label types into a uniform list
    struct LabelInfo {
        net: String,
        x: f64,
        y: f64,
    }
    let mut all_labels: Vec<LabelInfo> = Vec::new();
    for l in sch.labels.iter() {
        all_labels.push(LabelInfo {
            net: l.text.clone(),
            x: l.at.x,
            y: l.at.y,
        });
    }
    for g in sch.global_labels.iter() {
        all_labels.push(LabelInfo {
            net: g.text.clone(),
            x: g.at.x,
            y: g.at.y,
        });
    }
    for h in sch.hierarchical_labels.iter() {
        all_labels.push(LabelInfo {
            net: h.text.clone(),
            x: h.at.x,
            y: h.at.y,
        });
    }
    let mut label_overlaps: Vec<serde_json::Value> = Vec::new();
    for (i, a) in all_labels.iter().enumerate() {
        for b in &all_labels[i + 1..] {
            if points_coincident(a.x, a.y, b.x, b.y, tol) && a.net != b.net {
                label_overlaps.push(json!({ "type": "label_overlap", "net_a": a.net, "net_b": b.net, "x": a.x, "y": a.y }));
            }
        }
    }

    let mut all = comp_overlaps;
    all.extend(label_overlaps);
    Ok(CallToolResult::json(
        &json!({ "overlap_count": all.len(), "overlaps": all }),
    ))
}

// ─── Physically connected graphical islands ─────────────────────────────────

#[derive(Debug)]
pub(crate) struct IslandDsu {
    parent: Vec<usize>,
}

impl IslandDsu {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    pub(crate) fn find(&mut self, index: usize) -> usize {
        let parent = self.parent[index];
        if parent == index {
            index
        } else {
            let root = self.find(parent);
            self.parent[index] = root;
            root
        }
    }

    pub(crate) fn union(&mut self, first: usize, second: usize) {
        let first_root = self.find(first);
        let second_root = self.find(second);
        if first_root != second_root {
            self.parent[second_root] = first_root;
        }
    }
}

#[derive(Debug)]
struct RawLabel<'a> {
    node: &'a SexpNode,
    kind: &'static str,
    net: String,
    x: f64,
    y: f64,
}

#[derive(Debug)]
struct IslandAccum {
    bounds: super::sch_components::SchBounds,
    references: Vec<String>,
    labels: Vec<serde_json::Value>,
    wire_count: usize,
}

fn include_bounds(
    target: &mut super::sch_components::SchBounds,
    source: super::sch_components::SchBounds,
) {
    target.include(source.left, source.top);
    target.include(source.right, source.bottom);
}

fn label_rendered_bounds(node: &SexpNode) -> Option<super::sch_components::SchBounds> {
    let at = node.find("at")?;
    let x = at.get_f64(1)?;
    let y = at.get_f64(2)?;
    let rotation = at.get_f64(3).unwrap_or(0.0).rem_euclid(360.0);
    let text = node.get(1).and_then(SexpNode::as_str).unwrap_or("");
    let effects = node.find("effects");
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
    let text_width = text.chars().count().max(1) as f64 * font_x * 0.65;
    // Directional labels add a polygon around the text. The exact stroke-font
    // outline is renderer-dependent, so deliberately reserve a conservative
    // margin comparable to one default text height.
    let directional = matches!(node.head(), Some("global_label" | "hierarchical_label"));
    let horizontal_margin = if directional { font_x } else { 0.15 * font_x };
    let vertical_margin = if directional {
        0.35 * font_y
    } else {
        0.10 * font_y
    };
    let (text_left, text_right) = match justify {
        "left" => (0.0, text_width),
        "right" => (-text_width, 0.0),
        _ => (-text_width / 2.0, text_width / 2.0),
    };
    let local_left = text_left - horizontal_margin;
    let local_right = text_right + horizontal_margin;
    let local_top = -font_y / 2.0 - vertical_margin;
    let local_bottom = font_y / 2.0 + vertical_margin;
    let radians = rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let mut bounds = super::sch_components::SchBounds::empty();
    for (local_x, local_y) in [
        (local_left, local_top),
        (local_right, local_top),
        (local_left, local_bottom),
        (local_right, local_bottom),
    ] {
        bounds.include(
            x + local_x * cos - local_y * sin,
            y + local_x * sin + local_y * cos,
        );
    }
    bounds.valid().then_some(bounds)
}

pub(crate) fn wire_segments_touch(first: &Wire, second: &Wire, tolerance: f64) -> bool {
    [(first.x1, first.y1), (first.x2, first.y2)]
        .into_iter()
        .any(|(x, y)| point_on_segment(x, y, second.x1, second.y1, second.x2, second.y2, tolerance))
        || [(second.x1, second.y1), (second.x2, second.y2)]
            .into_iter()
            .any(|(x, y)| point_on_segment(x, y, first.x1, first.y1, first.x2, first.y2, tolerance))
}

fn bounds_conflict(
    first: super::sch_components::SchBounds,
    second: super::sch_components::SchBounds,
    clearance: f64,
) -> bool {
    first.left < second.right + clearance
        && first.right + clearance > second.left
        && first.top < second.bottom + clearance
        && first.bottom + clearance > second.top
}

async fn handle_get_connection_islands(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let sch_path = get_path(args, "schematic")?;
    let reference_filter = args.get("reference").and_then(|value| value.as_str());
    let clearance = opt_f64(args, "clearance").unwrap_or(2.54).max(0.0);
    let connect_tolerance = opt_f64(args, "connect_tolerance").unwrap_or(0.05).max(0.0);
    let (_, tree) = read_schematic(&sch_path)?;
    let instances = extract_symbol_instances(&tree);
    let wires = extract_wires(&tree);
    let lib_syms = tree
        .find("lib_symbols")
        .map(|node| node.find_all("symbol"))
        .unwrap_or_default();

    let mut raw_labels = Vec::new();
    for (head, kind) in [
        ("label", "NetLabel"),
        ("global_label", "GlobalLabel"),
        ("hierarchical_label", "HierarchicalLabel"),
    ] {
        for node in tree.find_all(head) {
            let Some(at) = node.find("at") else { continue };
            let (Some(x), Some(y)) = (at.get_f64(1), at.get_f64(2)) else {
                continue;
            };
            raw_labels.push(RawLabel {
                node,
                kind,
                net: node
                    .get(1)
                    .and_then(SexpNode::as_str)
                    .unwrap_or("")
                    .to_string(),
                x,
                y,
            });
        }
    }

    let symbol_base = 0usize;
    let wire_base = instances.len();
    let label_base = wire_base + wires.len();
    let mut dsu = IslandDsu::new(label_base + raw_labels.len());

    for (first_index, first) in wires.iter().enumerate() {
        for (second_index, second) in wires.iter().enumerate().skip(first_index + 1) {
            if wire_segments_touch(first, second, connect_tolerance) {
                dsu.union(wire_base + first_index, wire_base + second_index);
            }
        }
    }

    for (label_index, label) in raw_labels.iter().enumerate() {
        for (wire_index, wire) in wires.iter().enumerate() {
            if point_on_segment(
                label.x,
                label.y,
                wire.x1,
                wire.y1,
                wire.x2,
                wire.y2,
                connect_tolerance,
            ) {
                dsu.union(label_base + label_index, wire_base + wire_index);
            }
        }
    }

    for (symbol_index, instance) in instances.iter().enumerate() {
        let Some(lib_symbol) = super::resolve_embedded_pin_symbol(&lib_syms, &instance.lib_id)
        else {
            continue;
        };
        for pin in extract_lib_pins(lib_symbol) {
            let (pin_x, pin_y) = pin_endpoint(&pin, instance.pin_transform());
            for (wire_index, wire) in wires.iter().enumerate() {
                if point_on_segment(
                    pin_x,
                    pin_y,
                    wire.x1,
                    wire.y1,
                    wire.x2,
                    wire.y2,
                    connect_tolerance,
                ) {
                    dsu.union(symbol_base + symbol_index, wire_base + wire_index);
                }
            }
            for (label_index, label) in raw_labels.iter().enumerate() {
                if points_coincident(pin_x, pin_y, label.x, label.y, connect_tolerance) {
                    dsu.union(symbol_base + symbol_index, label_base + label_index);
                }
            }
        }
    }

    let mut islands: HashMap<usize, IslandAccum> = HashMap::new();
    for (index, instance) in instances.iter().enumerate() {
        let root = dsu.find(symbol_base + index);
        let island = islands.entry(root).or_insert_with(|| IslandAccum {
            bounds: super::sch_components::SchBounds::empty(),
            references: Vec::new(),
            labels: Vec::new(),
            wire_count: 0,
        });
        island.references.push(instance.reference.clone());
        if let Some(lib_symbol) = super::resolve_embedded_pin_symbol(&lib_syms, &instance.lib_id) {
            if let Some(body) = super::sch_components::transformed_body_bounds(lib_symbol, instance)
            {
                include_bounds(&mut island.bounds, body);
            }
            for pin in extract_lib_pins(lib_symbol) {
                let (pin_x, pin_y) = pin_endpoint(&pin, instance.pin_transform());
                island.bounds.include(pin_x, pin_y);
            }
        } else {
            island.bounds.include(instance.x, instance.y);
        }
        if let Some(instance_node) =
            super::sch_components::find_instance_node(&tree, &instance.reference)
        {
            for field_name in ["Reference", "Value"] {
                if let Some((field_bounds, ..)) = super::sch_components::property_bounds(
                    instance_node,
                    field_name,
                    instance.rotation,
                ) {
                    include_bounds(&mut island.bounds, field_bounds);
                }
            }
        }
    }

    for (index, wire) in wires.iter().enumerate() {
        let root = dsu.find(wire_base + index);
        let island = islands.entry(root).or_insert_with(|| IslandAccum {
            bounds: super::sch_components::SchBounds::empty(),
            references: Vec::new(),
            labels: Vec::new(),
            wire_count: 0,
        });
        island.bounds.include(wire.x1, wire.y1);
        island.bounds.include(wire.x2, wire.y2);
        island.wire_count += 1;
    }

    for (index, label) in raw_labels.iter().enumerate() {
        let root = dsu.find(label_base + index);
        let island = islands.entry(root).or_insert_with(|| IslandAccum {
            bounds: super::sch_components::SchBounds::empty(),
            references: Vec::new(),
            labels: Vec::new(),
            wire_count: 0,
        });
        if let Some(label_bounds) = label_rendered_bounds(label.node) {
            include_bounds(&mut island.bounds, label_bounds);
        } else {
            island.bounds.include(label.x, label.y);
        }
        island.labels.push(json!({
            "net": label.net,
            "type": label.kind,
            "x": label.x,
            "y": label.y
        }));
    }

    let mut island_list: Vec<IslandAccum> = islands
        .into_values()
        .filter(|island| island.bounds.valid())
        .collect();
    island_list.sort_by(|first, second| {
        first
            .bounds
            .top
            .total_cmp(&second.bounds.top)
            .then(first.bounds.left.total_cmp(&second.bounds.left))
    });
    for island in &mut island_list {
        island.references.sort();
    }

    let mut conflicts = Vec::new();
    for first_index in 0..island_list.len() {
        for second_index in (first_index + 1)..island_list.len() {
            let first = &island_list[first_index];
            let second = &island_list[second_index];
            if bounds_conflict(first.bounds, second.bounds, clearance) {
                let (distance, overlaps) =
                    super::sch_components::bounds_gap(first.bounds, second.bounds);
                conflicts.push(json!({
                    "island_a": first_index + 1,
                    "island_b": second_index + 1,
                    "a_references": first.references,
                    "b_references": second.references,
                    "aabb_distance": distance,
                    "aabb_overlaps": overlaps,
                    "required_clearance": clearance
                }));
            }
        }
    }

    let rendered: Vec<_> = island_list
        .iter()
        .enumerate()
        .filter(|(_, island)| {
            reference_filter
                .is_none_or(|reference| island.references.iter().any(|item| item == reference))
        })
        .map(|(index, island)| {
            json!({
                "id": index + 1,
                "bounds": island.bounds.json(),
                "references": island.references,
                "labels": island.labels,
                "wire_count": island.wire_count
            })
        })
        .collect();

    if reference_filter.is_some() && rendered.is_empty() {
        return Ok(CallToolResult::error(format!(
            "Component '{}' not found in any connection island",
            reference_filter.unwrap_or_default()
        )));
    }

    Ok(CallToolResult::json(&json!({
        "island_count": island_list.len(),
        "returned_count": rendered.len(),
        "islands": rendered,
        "clearance": clearance,
        "conflict_count": conflicts.len(),
        "conflicts": conflicts,
        "bounds_are_conservative": true,
        "connectivity_policy": "physical wires and symbol pins only; equal label names do not merge islands"
    })))
}

#[cfg(test)]
mod connection_island_tests {
    use super::{bounds_conflict, label_rendered_bounds, wire_segments_touch};
    use crate::tools::sch_components::SchBounds;
    use konnect_sexp::{parse_sexp, schematic::Wire};

    #[test]
    fn wires_join_at_a_t_endpoint_but_not_at_a_midspan_crossing() {
        let horizontal = Wire {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 0.0,
            uuid: None,
        };
        let tee = Wire {
            x1: 5.0,
            y1: 0.0,
            x2: 5.0,
            y2: 5.0,
            uuid: None,
        };
        let crossing = Wire {
            x1: 5.0,
            y1: -5.0,
            x2: 5.0,
            y2: 5.0,
            uuid: None,
        };
        assert!(wire_segments_touch(&horizontal, &tee, 0.01));
        assert!(!wire_segments_touch(&horizontal, &crossing, 0.01));
    }

    #[test]
    fn clearance_conflict_expands_both_island_boxes() {
        let first = SchBounds {
            left: 0.0,
            right: 10.0,
            top: 0.0,
            bottom: 10.0,
        };
        let second = SchBounds {
            left: 12.0,
            right: 20.0,
            top: 0.0,
            bottom: 10.0,
        };
        assert!(bounds_conflict(first, second, 2.54));
        assert!(!bounds_conflict(first, second, 1.0));
    }

    #[test]
    fn directional_label_bounds_include_more_than_the_anchor() {
        let tree = parse_sexp("(kicad_sch (global_label \"LONG_NET\" (shape bidirectional) (at 10 20 180) (effects (font (size 1.27 1.27)) (justify right))))").unwrap();
        let label = tree.find("global_label").unwrap();
        let bounds = label_rendered_bounds(label).unwrap();
        assert!(bounds.right - bounds.left > 5.0);
        assert!(bounds.bottom - bounds.top > 1.27);
    }
}
