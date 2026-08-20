//! Safe, high-level design workflows.
//!
//! The public contract is deliberately expressed as typed design operations,
//! not as a list of raw MCP tool calls.  A plan is read-only, carries a
//! resource fingerprint, and can only be applied while that fingerprint is
//! still current.

use crate::mcp::error::ToolErrorKind;
use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{ToolContext, ToolDef};
use konnect_ipc::client::IpcError;
use konnect_ipc::types::{IpcFootprint, IpcFootprintCourtyard, IpcFootprintTransform, IpcVector2};
use konnect_sexp::geometry::snap_point;
use konnect_sexp::writer::{
    apply_edits, find_block_with_leading_whitespace, write_atomic, SexpEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

const WORKFLOW_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_WORKFLOWS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDomain {
    Schematic,
    Pcb,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Planned,
    Applying,
    Applied,
    Verified,
    Stale,
    Rejected,
    VerificationFailed,
    Failed,
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    None,
    PersistedToDisk,
    LiveDocument,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowOperation {
    EditComponents {
        edits: Vec<ComponentEdit>,
    },
    MoveComponents {
        references: Vec<String>,
        dx: f64,
        dy: f64,
    },
    TransformFootprint {
        reference: String,
        x: Option<f64>,
        y: Option<f64>,
        rotation: Option<f64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentEdit {
    pub reference: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub footprint: Option<String>,
    #[serde(default)]
    pub in_bom: Option<bool>,
    #[serde(default)]
    pub dnp: Option<bool>,
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDiff {
    pub before_sha256: String,
    pub after_sha256: String,
    pub changes: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    pub resource: String,
    pub base_sha256: String,
    pub operations: Vec<WorkflowOperation>,
    pub allow_list: Vec<String>,
    pub expected_diff: Option<DesignDiff>,
    /// Pre-existing validation findings accepted as the baseline. Verification
    /// rejects only newly introduced findings.
    pub validation_baseline: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub api_version: u32,
    pub domain: WorkflowDomain,
    pub lifecycle: LifecycleState,
    pub effect_state: EffectState,
    pub plan: EditPlan,
    pub actual_diff: Option<DesignDiff>,
    pub error: Option<WorkflowError>,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

fn workflow_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
) -> Option<WorkflowError> {
    Some(WorkflowError {
        code: code.into(),
        message: message.into(),
        retryable,
    })
}

fn expired_change_set(id: &str) -> CallToolResult {
    CallToolResult::error_kind(
        ToolErrorKind::ChangeSetExpired {
            change_set_id: id.to_owned(),
        },
        format!("change set '{id}' was not found or expired"),
    )
}

fn invalid_state(state: &LifecycleState, message: impl Into<String>) -> CallToolResult {
    CallToolResult::error_kind(
        ToolErrorKind::InvalidWorkflowState {
            state: format!("{state:?}").to_lowercase(),
        },
        message,
    )
}

#[derive(Default)]
struct WorkflowStore {
    runs: Mutex<HashMap<String, WorkflowRun>>,
    locks: Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
}

impl WorkflowStore {
    fn insert(&self, run: WorkflowRun) {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs);
        if runs.len() >= MAX_WORKFLOWS {
            if let Some(oldest) = runs
                .values()
                .filter(|run| {
                    matches!(
                        run.effect_state,
                        EffectState::None | EffectState::PersistedToDisk
                    )
                })
                .min_by_key(|run| run.created_at_unix)
                .map(|run| run.id.clone())
            {
                runs.remove(&oldest);
            }
        }
        runs.insert(run.id.clone(), run);
    }

    fn get(&self, id: &str) -> Option<WorkflowRun> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs);
        runs.get(id).cloned()
    }

    fn update(&self, run: WorkflowRun) {
        self.runs
            .lock()
            .expect("workflow store poisoned")
            .insert(run.id.clone(), run);
    }

    fn resource_lock(&self, resource: &Path) -> Arc<AsyncMutex<()>> {
        let mut locks = self.locks.lock().expect("workflow lock map poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(resource).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(resource.to_path_buf(), Arc::downgrade(&lock));
        lock
    }

    fn purge_expired(runs: &mut HashMap<String, WorkflowRun>) {
        let now = unix_now();
        // Never forget an unsaved or outcome-unknown live edit: the caller
        // still needs verify/recovery information even after the plan TTL.
        runs.retain(|_, run| {
            run.expires_at_unix > now
                || matches!(
                    run.effect_state,
                    EffectState::LiveDocument | EffectState::Unknown
                )
        });
    }
}

fn store() -> &'static WorkflowStore {
    static STORE: OnceLock<WorkflowStore> = OnceLock::new();
    STORE.get_or_init(WorkflowStore::default)
}

pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "inspect_design",
            "Inspect a KiCad project, schematic, or PCB through one read-only workflow view.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "A .kicad_pro, .kicad_sch, .kicad_pcb, or project directory" },
                    "schematic": { "type": "string" },
                    "board": { "type": "string" }
                }
            }),
            |args, ctx| async move { handle_inspect(args, ctx).await }
        ),
        tool!(
            "plan_schematic_edit",
            "Validate schematic component edits and return an immutable, zero-write change set.",
            schematic_plan_schema(),
            |args, ctx| async move { handle_plan_schematic(args, ctx).await }
        ),
        tool!(
            "plan_pcb_edit",
            "Preflight typed live-PCB footprint transforms, fingerprints, and projected courtyards without writing.",
            pcb_plan_schema(),
            |args, ctx| async move { handle_plan_pcb(args, ctx).await }
        ),
        tool!(
            "get_change_set",
            "Read the complete machine-readable state of a planned workflow.",
            id_schema(),
            |args, ctx| async move { handle_get(args, ctx).await }
        ),
        tool!(
            "apply_change_set",
            "Apply a non-stale supported change set once, under a canonical resource lock.",
            id_schema(),
            |args, ctx| async move { handle_apply(args, ctx).await }
        ),
        tool!(
            "verify_change_set",
            "Verify an applied resource against its planned result; save a live PCB only after all checks pass.",
            id_schema(),
            |args, ctx| async move { handle_verify(args, ctx).await }
        ),
        tool!(
            "discard_change_set",
            "Discard a plan that has not produced persisted effects.",
            id_schema(),
            |args, ctx| async move { handle_discard(args, ctx).await }
        ),
    ]
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "change_set_id": { "type": "string", "format": "uuid" } },
        "required": ["change_set_id"],
        "additionalProperties": false
    })
}

fn schematic_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "schematic": { "type": "string" },
            "operations": {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "edit_components" },
                                "edits": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "reference": { "type": "string", "minLength": 1 },
                                            "value": { "type": "string" },
                                            "footprint": { "type": "string" },
                                            "in_bom": { "type": "boolean" },
                                            "dnp": { "type": "boolean" },
                                            "fields": {
                                                "type": "object",
                                                "additionalProperties": { "type": "string" }
                                            }
                                        },
                                        "required": ["reference"],
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "required": ["kind", "edits"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "move_components" },
                                "references": { "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 } },
                                "dx": { "type": "number" },
                                "dy": { "type": "number" }
                            },
                            "required": ["kind", "references", "dx", "dy"],
                            "additionalProperties": false
                        }
                    ]
                },
                "minItems": 1
            }
        },
        "required": ["schematic", "operations"],
        "additionalProperties": false
    })
}

fn pcb_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "board": { "type": "string" },
            "operations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "const": "transform_footprint" },
                        "reference": { "type": "string" },
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "rotation": { "type": "number" }
                    },
                    "required": ["kind", "reference"],
                    "anyOf": [
                        { "required": ["x", "y"] },
                        { "required": ["rotation"] }
                    ],
                    "additionalProperties": false
                },
                "minItems": 1
            }
        },
        "required": ["board", "operations"],
        "additionalProperties": false
    })
}

struct LivePcbSnapshot {
    contents: String,
    footprints: Vec<IpcFootprint>,
    courtyards: Vec<IpcFootprintCourtyard>,
}

async fn live_pcb_snapshot(ctx: &ToolContext, board: PathBuf) -> anyhow::Result<LivePcbSnapshot> {
    let client = ctx.ipc.clone();
    tokio::task::spawn_blocking(move || {
        let client = client.bind_board(&board)?;
        Ok(LivePcbSnapshot {
            contents: client.read_live_board()?,
            footprints: client.list_footprints()?,
            courtyards: client.list_footprint_courtyards()?,
        })
    })
    .await
    .map_err(|error| anyhow::anyhow!("PCB snapshot worker failed: {error}"))?
}

fn ipc_transforms(operations: &[WorkflowOperation]) -> anyhow::Result<Vec<IpcFootprintTransform>> {
    let mut transforms = Vec::with_capacity(operations.len());
    let mut references = std::collections::HashSet::with_capacity(operations.len());
    for operation in operations {
        let WorkflowOperation::TransformFootprint {
            reference,
            x,
            y,
            rotation,
        } = operation
        else {
            anyhow::bail!("PCB plans only accept transform_footprint operations");
        };
        validate_reference(reference)?;
        if !references.insert(reference.as_str()) {
            anyhow::bail!("footprint '{reference}' appears more than once");
        }
        if x.is_some() != y.is_some() {
            anyhow::bail!("footprint '{reference}' must provide both x and y");
        }
        if x.is_none() && rotation.is_none() {
            anyhow::bail!("footprint '{reference}' transform contains no changes");
        }
        if x.is_some_and(|value| !value.is_finite())
            || y.is_some_and(|value| !value.is_finite())
            || rotation.is_some_and(|value| !value.is_finite())
        {
            anyhow::bail!("footprint '{reference}' transform contains a non-finite number");
        }
        transforms.push(IpcFootprintTransform {
            reference: reference.clone(),
            position: match (*x, *y) {
                (Some(x), Some(y)) => Some(IpcVector2 { x, y }),
                _ => None,
            },
            rotation: *rotation,
        });
    }
    Ok(transforms)
}

fn projected_courtyards(
    snapshot: &LivePcbSnapshot,
    transforms: &[IpcFootprintTransform],
) -> anyhow::Result<Vec<IpcFootprintCourtyard>> {
    let footprints: HashMap<_, _> = snapshot
        .footprints
        .iter()
        .map(|footprint| (footprint.reference.as_str(), footprint))
        .collect();
    let transforms: HashMap<_, _> = transforms
        .iter()
        .map(|transform| (transform.reference.as_str(), transform))
        .collect();
    let mut projected = snapshot.courtyards.clone();
    for courtyard in &mut projected {
        let Some(transform) = transforms.get(courtyard.reference.as_str()) else {
            continue;
        };
        let footprint = footprints
            .get(courtyard.reference.as_str())
            .ok_or_else(|| anyhow::anyhow!("footprint '{}' was not found", courtyard.reference))?;
        let old = &footprint.position;
        let target = transform.position.as_ref().unwrap_or(old);
        let delta = transform.rotation.unwrap_or(footprint.rotation) - footprint.rotation;
        let radians = delta.to_radians();
        let (sin, cos) = radians.sin_cos();
        for primitive in &mut courtyard.primitives {
            for point in &mut primitive.points {
                let dx = point.x - old.x;
                let dy = point.y - old.y;
                point.x = target.x + dx * cos - dy * sin;
                point.y = target.y + dx * sin + dy * cos;
            }
        }
        let mut points = courtyard
            .primitives
            .iter()
            .flat_map(|primitive| primitive.points.iter());
        courtyard.bounds = points.next().map(|first| {
            let mut bounds = konnect_ipc::types::IpcBounds {
                min: first.clone(),
                max: first.clone(),
            };
            for point in points {
                bounds.min.x = bounds.min.x.min(point.x);
                bounds.min.y = bounds.min.y.min(point.y);
                bounds.max.x = bounds.max.x.max(point.x);
                bounds.max.y = bounds.max.y.max(point.y);
            }
            bounds
        });
    }
    Ok(projected)
}

fn validate_pcb_targets(
    snapshot: &LivePcbSnapshot,
    transforms: &[IpcFootprintTransform],
) -> anyhow::Result<()> {
    for transform in transforms {
        let footprint_count = snapshot
            .footprints
            .iter()
            .filter(|footprint| footprint.reference == transform.reference)
            .count();
        if footprint_count != 1 {
            anyhow::bail!(
                "footprint reference '{}' must resolve exactly once (found {})",
                transform.reference,
                footprint_count
            );
        }
        let courtyards: Vec<_> = snapshot
            .courtyards
            .iter()
            .filter(|courtyard| courtyard.reference == transform.reference)
            .collect();
        if courtyards.is_empty()
            || courtyards.iter().any(|courtyard| {
                courtyard.bounds.is_none()
                    || courtyard
                        .primitives
                        .iter()
                        .all(|primitive| primitive.points.len() < 2)
            })
        {
            anyhow::bail!(
                "footprint '{}' has no usable courtyard geometry",
                transform.reference
            );
        }
    }
    Ok(())
}

fn courtyard_issues(courtyards: &[IpcFootprintCourtyard]) -> HashSet<String> {
    let mut issues = HashSet::new();
    for (index, a) in courtyards.iter().enumerate() {
        for b in &courtyards[index + 1..] {
            if a.reference == b.reference || a.layer != b.layer {
                continue;
            }
            let (Some(a_bounds), Some(b_bounds)) = (&a.bounds, &b.bounds) else {
                continue;
            };
            if !super::pcb_components::bounds_overlap(a_bounds, b_bounds, 0.0) {
                continue;
            }
            match super::pcb_components::exact_courtyard_conflict(a, b, 0.0) {
                Some(false) => {}
                Some(true) => {
                    let (first, second) = if a.reference <= b.reference {
                        (&a.reference, &b.reference)
                    } else {
                        (&b.reference, &a.reference)
                    };
                    issues.insert(format!("overlap:{}:{}:{}", first, second, a.layer));
                }
                None => {
                    let (first, second) = if a.reference <= b.reference {
                        (&a.reference, &b.reference)
                    } else {
                        (&b.reference, &a.reference)
                    };
                    issues.insert(format!("inconclusive:{}:{}:{}", first, second, a.layer));
                }
            }
        }
    }
    issues
}

fn reject_new_courtyard_issues(
    courtyards: &[IpcFootprintCourtyard],
    baseline: &HashSet<String>,
) -> anyhow::Result<()> {
    let new_issues: Vec<_> = courtyard_issues(courtyards)
        .difference(baseline)
        .cloned()
        .collect();
    if !new_issues.is_empty() {
        anyhow::bail!(
            "footprint transforms introduce new courtyard findings: {}",
            new_issues.join(", ")
        );
    }
    Ok(())
}

async fn handle_inspect(args: &Value, ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let (project, schematic, board) = resolve_design_paths(args)?;
    let project_info = project.as_ref().map(inspect_file);
    let schematic_info = match schematic.as_ref() {
        Some(path) if path.exists() => {
            let parsed = konnect_schematic_editor::Schematic::load(path)?;
            Some(json!({
                "path": path.display().to_string(),
                "sha256": hash_file(path)?,
                "symbols": parsed.symbols.iter().count(),
                "wires": parsed.wires.iter().count(),
                "labels": parsed.labels.iter().count(),
                "global_labels": parsed.global_labels.iter().count(),
                "junctions": parsed.junctions.len(),
                "sheets": parsed.sheets.iter().count()
            }))
        }
        Some(path) => Some(json!({ "path": path.display().to_string(), "exists": false })),
        None => None,
    };
    let board_info = match board.as_ref() {
        Some(path) if path.exists() => {
            let bytes = std::fs::read(path)?;
            let disk_text = String::from_utf8_lossy(&bytes);
            let mut info = json!({
                "path": path.display().to_string(),
                "disk_sha256": sha256(&bytes),
                "disk_bytes": bytes.len(),
                "footprints": disk_text.matches("\n  (footprint ").count(),
                "segments": disk_text.matches("\n  (segment ").count(),
                "vias": disk_text.matches("\n  (via ").count(),
                "zones": disk_text.matches("\n  (zone ").count(),
                "live_open": false
            });
            match live_pcb_snapshot(ctx, path.clone()).await {
                Ok(live) => {
                    info["live_open"] = json!(true);
                    info["source"] = json!("kicad_ipc_live_document");
                    info["live_sha256"] = json!(sha256(live.contents.as_bytes()));
                    info["live_footprints"] = json!(live.footprints.len());
                    info["live_courtyards"] = json!(live.courtyards.len());
                }
                Err(error) => {
                    info["source"] = json!("filesystem_snapshot");
                    info["live_error"] = json!(error.to_string());
                }
            }
            Some(info)
        }
        Some(path) => Some(json!({ "path": path.display().to_string(), "exists": false })),
        None => None,
    };
    Ok(CallToolResult::json(&json!({
        "source": "aggregate_design_snapshot",
        "project": project_info,
        "schematic": schematic_info,
        "board": board_info
    })))
}

async fn handle_plan_schematic(args: &Value, _ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let resource = canonical_resource(args, "schematic", "kicad_sch")?;
    let operations: Vec<WorkflowOperation> = serde_json::from_value(
        args.get("operations")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing operations"))?,
    )?;
    if operations.is_empty() {
        anyhow::bail!("operations must not be empty");
    }
    if operations
        .iter()
        .any(|op| matches!(op, WorkflowOperation::TransformFootprint { .. }))
    {
        anyhow::bail!("PCB operations are not valid in a schematic plan");
    }
    let original = std::fs::read_to_string(&resource)?;
    let transformed = transform_schematic(&original, &operations)?;
    // Parsing here proves plan/dry-run validity without writing the file.
    parse_schematic_text(&transformed.content, &resource)?;
    let before_sha256 = sha256(original.as_bytes());
    let after_sha256 = sha256(transformed.content.as_bytes());
    let allow_list = allow_list(&operations);
    let run = new_run(
        WorkflowDomain::Schematic,
        resource,
        operations,
        allow_list,
        Vec::new(),
        Some(DesignDiff {
            before_sha256: before_sha256.clone(),
            after_sha256,
            changes: transformed.changes,
        }),
        before_sha256,
    );
    store().insert(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn handle_plan_pcb(args: &Value, ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let resource = canonical_resource(args, "board", "kicad_pcb")?;
    let operations: Vec<WorkflowOperation> = serde_json::from_value(
        args.get("operations")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing operations"))?,
    )?;
    if operations.is_empty() {
        anyhow::bail!("PCB plans require one or more transform_footprint operations");
    }
    let transforms = ipc_transforms(&operations)?;
    let snapshot = live_pcb_snapshot(ctx, resource.clone()).await?;
    validate_pcb_targets(&snapshot, &transforms)?;
    let baseline = courtyard_issues(&snapshot.courtyards);
    reject_new_courtyard_issues(&projected_courtyards(&snapshot, &transforms)?, &baseline)?;
    let mut validation_baseline: Vec<_> = baseline.into_iter().collect();
    validation_baseline.sort();
    let base = sha256(snapshot.contents.as_bytes());
    let run = new_run(
        WorkflowDomain::Pcb,
        resource,
        operations.clone(),
        allow_list(&operations),
        validation_baseline,
        None,
        base,
    );
    store().insert(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn handle_get(args: &Value, _ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let id = required_id(args)?;
    Ok(match store().get(id) {
        Some(run) => CallToolResult::json(&run),
        None => expired_change_set(id),
    })
}

async fn handle_apply(args: &Value, ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let id = required_id(args)?;
    let initial = match store().get(id) {
        Some(run) => run,
        None => return Ok(expired_change_set(id)),
    };
    let resource = PathBuf::from(&initial.plan.resource);
    let lock = store().resource_lock(&resource);
    let _guard = lock.lock().await;
    // Another request may have completed while this request waited for the
    // resource lock. Re-read the state so retries stay idempotent.
    let mut run = match store().get(id) {
        Some(run) => run,
        None => return Ok(expired_change_set(id)),
    };
    if matches!(
        run.lifecycle,
        LifecycleState::Applied | LifecycleState::Verified
    ) {
        return Ok(CallToolResult::json(&run));
    }
    if run.lifecycle != LifecycleState::Planned {
        return Ok(invalid_state(
            &run.lifecycle,
            format!(
                "change set cannot be applied from state {:?}",
                run.lifecycle
            ),
        ));
    }
    run.lifecycle = LifecycleState::Applying;
    store().update(run.clone());

    if run.domain == WorkflowDomain::Pcb {
        return apply_pcb_change_set(run, resource, ctx).await;
    }

    let original = match std::fs::read_to_string(&resource) {
        Ok(content) => content,
        Err(error) => {
            run.lifecycle = LifecycleState::Rejected;
            run.error = workflow_error(
                "apply_read_failed",
                format!("cannot read schematic before apply: {error}"),
                true,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let current_sha = sha256(original.as_bytes());
    if current_sha != run.plan.base_sha256 {
        run.lifecycle = LifecycleState::Stale;
        run.error = workflow_error(
            "stale_revision",
            format!(
                "resource changed after planning: expected {}, found {}",
                run.plan.base_sha256, current_sha
            ),
            false,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    let transformed = match transform_schematic(&original, &run.plan.operations) {
        Ok(result) => result,
        Err(error) => {
            run.lifecycle = LifecycleState::Rejected;
            run.error = workflow_error("invalid_plan", error.to_string(), false);
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    if let Err(error) = parse_schematic_text(&transformed.content, &resource) {
        run.lifecycle = LifecycleState::Rejected;
        run.error = workflow_error(
            "invalid_plan",
            format!("planned schematic no longer parses: {error}"),
            false,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    if hash_file(&resource).ok().as_deref() != Some(run.plan.base_sha256.as_str()) {
        run.lifecycle = LifecycleState::Stale;
        run.error = workflow_error(
            "stale_revision",
            "schematic changed during apply preflight; no workflow write was performed",
            false,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    if let Err(error) = write_atomic(&resource, &transformed.content) {
        run.lifecycle = LifecycleState::Failed;
        run.error = workflow_error(
            "apply_failed",
            format!("atomic schematic write failed: {error}"),
            true,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    let after_sha = match hash_file(&resource) {
        Ok(hash) => hash,
        Err(error) => {
            run.lifecycle = LifecycleState::Failed;
            run.effect_state = EffectState::Unknown;
            run.error = workflow_error(
                "partial_apply",
                format!("schematic was written but could not be re-read: {error}"),
                false,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    run.actual_diff = Some(DesignDiff {
        before_sha256: current_sha,
        after_sha256: after_sha,
        changes: transformed.changes,
    });
    run.lifecycle = LifecycleState::Applied;
    run.effect_state = EffectState::PersistedToDisk;
    run.error = None;
    store().update(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn apply_pcb_change_set(
    mut run: WorkflowRun,
    resource: PathBuf,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let snapshot = match live_pcb_snapshot(ctx, resource.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            run.lifecycle = LifecycleState::Rejected;
            run.error = workflow_error(
                "apply_read_failed",
                format!("cannot inspect live PCB before apply: {error}"),
                true,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let current_sha = sha256(snapshot.contents.as_bytes());
    if current_sha != run.plan.base_sha256 {
        run.lifecycle = LifecycleState::Stale;
        run.error = workflow_error(
            "stale_revision",
            format!(
                "live PCB changed after planning: expected {}, found {}",
                run.plan.base_sha256, current_sha
            ),
            false,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    let transforms = match ipc_transforms(&run.plan.operations).and_then(|transforms| {
        let baseline: HashSet<_> = run.plan.validation_baseline.iter().cloned().collect();
        validate_pcb_targets(&snapshot, &transforms)?;
        reject_new_courtyard_issues(&projected_courtyards(&snapshot, &transforms)?, &baseline)?;
        Ok(transforms)
    }) {
        Ok(transforms) => transforms,
        Err(error) => {
            run.lifecycle = LifecycleState::Rejected;
            run.error = workflow_error("invalid_plan", error.to_string(), false);
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let client = ctx.ipc.clone();
    let board = resource.clone();
    let apply_result = match tokio::task::spawn_blocking(move || {
        client
            .bind_board(board)?
            .transform_footprints_atomically(&transforms)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            run.lifecycle = LifecycleState::Failed;
            run.effect_state = EffectState::Unknown;
            run.error = workflow_error(
                "partial_apply",
                format!("PCB apply worker failed: {error}"),
                false,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let states = match apply_result {
        Ok(states) => states,
        Err(error) => {
            if error
                .downcast_ref::<IpcError>()
                .is_some_and(|error| matches!(error, IpcError::OutcomeUnknown { .. }))
            {
                run.effect_state = EffectState::Unknown;
            }
            run.lifecycle = LifecycleState::Failed;
            run.error = workflow_error(
                if run.effect_state == EffectState::Unknown {
                    "partial_apply"
                } else {
                    "apply_failed"
                },
                format!("PCB transaction failed: {error}"),
                false,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let after = match live_pcb_snapshot(ctx, resource).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            run.lifecycle = LifecycleState::Failed;
            run.effect_state = EffectState::Unknown;
            run.error = workflow_error(
                "partial_apply",
                format!("PCB was changed but cannot be re-read: {error}"),
                false,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    run.actual_diff = Some(DesignDiff {
        before_sha256: current_sha,
        after_sha256: sha256(after.contents.as_bytes()),
        changes: states
            .iter()
            .map(|state| {
                json!({
                    "kind": "footprint_transformed",
                    "reference": state.reference,
                    "after": state
                })
            })
            .collect(),
    });
    run.lifecycle = LifecycleState::Applied;
    run.effect_state = EffectState::LiveDocument;
    run.error = None;
    store().update(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn handle_verify(args: &Value, _ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let id = required_id(args)?;
    let mut run = match store().get(id) {
        Some(run) => run,
        None => return Ok(expired_change_set(id)),
    };
    if run.lifecycle == LifecycleState::Verified {
        return Ok(CallToolResult::json(&run));
    }
    if run.lifecycle != LifecycleState::Applied
        && !(run.lifecycle == LifecycleState::VerificationFailed
            && run.effect_state != EffectState::Unknown)
    {
        return Ok(invalid_state(
            &run.lifecycle,
            "only an applied change set can be verified",
        ));
    }
    let resource = PathBuf::from(&run.plan.resource);
    let lock = store().resource_lock(&resource);
    let _guard = lock.lock().await;
    // Re-read after waiting: apply/verify retries may have completed already.
    run = match store().get(id) {
        Some(current) => current,
        None => return Ok(expired_change_set(id)),
    };
    if run.lifecycle == LifecycleState::Verified {
        return Ok(CallToolResult::json(&run));
    }
    if run.lifecycle != LifecycleState::Applied
        && !(run.lifecycle == LifecycleState::VerificationFailed
            && run.effect_state != EffectState::Unknown)
    {
        return Ok(invalid_state(
            &run.lifecycle,
            "only an applied change set can be verified",
        ));
    }
    if run.domain == WorkflowDomain::Pcb {
        return verify_pcb_change_set(run, resource, _ctx).await;
    }
    let content = match std::fs::read_to_string(&resource) {
        Ok(content) => content,
        Err(error) => {
            run.lifecycle = LifecycleState::VerificationFailed;
            run.error = workflow_error(
                "verification_failed",
                format!("cannot read schematic for verification: {error}"),
                true,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let parse_result = parse_schematic_text(&content, &resource);
    let actual_sha = sha256(content.as_bytes());
    let expected_sha = run
        .plan
        .expected_diff
        .as_ref()
        .map(|diff| diff.after_sha256.as_str());
    if parse_result.is_ok() && expected_sha == Some(actual_sha.as_str()) {
        run.lifecycle = LifecycleState::Verified;
        run.error = None;
    } else {
        run.lifecycle = LifecycleState::VerificationFailed;
        run.error = workflow_error(
            "verification_failed",
            match parse_result {
                Err(error) => format!("schematic parse failed: {error}"),
                Ok(_) => format!(
                    "post-apply fingerprint mismatch: expected {}, found {}",
                    expected_sha.unwrap_or("<none>"),
                    actual_sha
                ),
            },
            false,
        );
    }
    store().update(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn verify_pcb_change_set(
    mut run: WorkflowRun,
    resource: PathBuf,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let snapshot = match live_pcb_snapshot(ctx, resource.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            run.lifecycle = LifecycleState::VerificationFailed;
            run.error = workflow_error(
                "verification_failed",
                format!("cannot inspect live PCB for verification: {error}"),
                true,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    let expected_hash = run
        .actual_diff
        .as_ref()
        .map(|diff| diff.after_sha256.as_str());
    let actual_hash = sha256(snapshot.contents.as_bytes());
    let transforms = ipc_transforms(&run.plan.operations)?;
    let footprints: HashMap<_, _> = snapshot
        .footprints
        .iter()
        .map(|footprint| (footprint.reference.as_str(), footprint))
        .collect();
    let states_match = transforms.iter().all(|transform| {
        footprints
            .get(transform.reference.as_str())
            .is_some_and(|footprint| {
                let position_matches = transform.position.as_ref().is_none_or(|target| {
                    (footprint.position.x - target.x).abs() <= 1e-6
                        && (footprint.position.y - target.y).abs() <= 1e-6
                });
                let rotation_matches = transform
                    .rotation
                    .is_none_or(|target| (footprint.rotation - target).abs() <= 1e-6);
                position_matches && rotation_matches
            })
    });
    let baseline: HashSet<_> = run.plan.validation_baseline.iter().cloned().collect();
    let courtyard_result = reject_new_courtyard_issues(&snapshot.courtyards, &baseline);
    if expected_hash != Some(actual_hash.as_str()) || !states_match || courtyard_result.is_err() {
        run.lifecycle = LifecycleState::VerificationFailed;
        run.error = workflow_error(
            "verification_failed",
            if expected_hash != Some(actual_hash.as_str()) {
                format!(
                    "live PCB fingerprint changed after apply: expected {}, found {}",
                    expected_hash.unwrap_or("<none>"),
                    actual_hash
                )
            } else if !states_match {
                "one or more footprint transforms do not match the requested absolute state".into()
            } else {
                courtyard_result.unwrap_err().to_string()
            },
            false,
        );
        store().update(run.clone());
        return Ok(CallToolResult::json(&run));
    }
    let client = ctx.ipc.clone();
    let save_result = match tokio::task::spawn_blocking(move || {
        client.bind_board(resource)?.save_board()
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            run.lifecycle = LifecycleState::VerificationFailed;
            run.effect_state = EffectState::Unknown;
            run.error = workflow_error(
                "save_failed",
                format!("PCB save worker failed: {error}"),
                false,
            );
            store().update(run.clone());
            return Ok(CallToolResult::json(&run));
        }
    };
    if let Err(error) = save_result {
        run.lifecycle = LifecycleState::VerificationFailed;
        run.effect_state = if error
            .downcast_ref::<IpcError>()
            .is_some_and(|error| matches!(error, IpcError::OutcomeUnknown { .. }))
        {
            EffectState::Unknown
        } else {
            EffectState::LiveDocument
        };
        run.error = workflow_error(
            "save_failed",
            format!("PCB verified but save failed: {error}"),
            true,
        );
    } else {
        run.lifecycle = LifecycleState::Verified;
        run.effect_state = EffectState::PersistedToDisk;
        run.error = None;
    }
    store().update(run.clone());
    Ok(CallToolResult::json(&run))
}

async fn handle_discard(args: &Value, _ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let id = required_id(args)?;
    let initial = match store().get(id) {
        Some(run) => run,
        None => return Ok(expired_change_set(id)),
    };
    let resource = PathBuf::from(&initial.plan.resource);
    let lock = store().resource_lock(&resource);
    let _guard = lock.lock().await;
    let mut run = match store().get(id) {
        Some(run) => run,
        None => return Ok(expired_change_set(id)),
    };
    if run.lifecycle == LifecycleState::Discarded {
        return Ok(CallToolResult::json(&run));
    }
    if run.lifecycle != LifecycleState::Planned || run.effect_state != EffectState::None {
        return Ok(invalid_state(
            &run.lifecycle,
            "only a planned change set with no effects can be discarded",
        ));
    }
    run.lifecycle = LifecycleState::Discarded;
    run.error = None;
    store().update(run.clone());
    Ok(CallToolResult::json(&run))
}

fn new_run(
    domain: WorkflowDomain,
    resource: PathBuf,
    operations: Vec<WorkflowOperation>,
    allow_list: Vec<String>,
    validation_baseline: Vec<String>,
    expected_diff: Option<DesignDiff>,
    base_sha256: String,
) -> WorkflowRun {
    let created = unix_now();
    WorkflowRun {
        id: Uuid::new_v4().to_string(),
        api_version: 1,
        domain,
        lifecycle: LifecycleState::Planned,
        effect_state: EffectState::None,
        plan: EditPlan {
            resource: resource.display().to_string(),
            base_sha256,
            operations,
            allow_list,
            expected_diff,
            validation_baseline,
        },
        actual_diff: None,
        error: None,
        created_at_unix: created,
        expires_at_unix: created + WORKFLOW_TTL.as_secs(),
    }
}

struct TransformResult {
    content: String,
    changes: Vec<Value>,
}

fn transform_schematic(
    content: &str,
    operations: &[WorkflowOperation],
) -> anyhow::Result<TransformResult> {
    let mut edits = Vec::new();
    let mut changes = Vec::new();
    for operation in operations {
        match operation {
            WorkflowOperation::EditComponents {
                edits: component_edits,
            } => {
                for edit in component_edits {
                    validate_reference(&edit.reference)?;
                    let mut changed_fields = Vec::new();
                    for (field, value) in [
                        ("Value", edit.value.as_ref()),
                        ("Footprint", edit.footprint.as_ref()),
                    ] {
                        if let Some(value) = value {
                            let (start, end) = field_value_range(content, &edit.reference, field)
                                .ok_or_else(|| {
                                anyhow::anyhow!("field '{field}' not found on '{}'", edit.reference)
                            })?;
                            edits.push(SexpEdit::replace(start, end, escape_quoted_value(value)));
                            changed_fields.push(json!({ "field": field, "value": value }));
                        }
                    }
                    for (field, value) in [("in_bom", edit.in_bom), ("dnp", edit.dnp)] {
                        if let Some(value) = value {
                            let (start, end) = symbol_scalar_range(content, &edit.reference, field)
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "attribute '{field}' not found on '{}'",
                                        edit.reference
                                    )
                                })?;
                            edits.push(SexpEdit::replace(
                                start,
                                end,
                                if value { "yes" } else { "no" },
                            ));
                            changed_fields.push(json!({ "field": field, "value": value }));
                        }
                    }
                    for (field, value) in &edit.fields {
                        if ["reference", "value", "footprint"]
                            .iter()
                            .any(|reserved| field.eq_ignore_ascii_case(reserved))
                        {
                            anyhow::bail!(
                                "field '{field}' is reserved; use the typed component property"
                            );
                        }
                        let (start, end) = field_value_range(content, &edit.reference, field)
                            .ok_or_else(|| {
                                anyhow::anyhow!("field '{field}' not found on '{}'", edit.reference)
                            })?;
                        edits.push(SexpEdit::replace(start, end, escape_quoted_value(value)));
                        changed_fields.push(json!({ "field": field, "value": value }));
                    }
                    if changed_fields.is_empty() {
                        anyhow::bail!(
                            "component edit for '{}' contains no changes",
                            edit.reference
                        );
                    }
                    changes.push(json!({
                        "kind": "component_fields_changed",
                        "reference": edit.reference,
                        "fields": changed_fields
                    }));
                }
            }
            WorkflowOperation::MoveComponents { references, dx, dy } => {
                if !dx.is_finite() || !dy.is_finite() {
                    anyhow::bail!("move offsets must be finite numbers");
                }
                if references.is_empty() {
                    anyhow::bail!("move_components requires at least one reference");
                }
                for reference in references {
                    validate_reference(reference)?;
                    let (sym_start, sym_end) = find_symbol_block(content, reference)
                        .ok_or_else(|| anyhow::anyhow!("component '{reference}' not found"))?;
                    let block = &content[sym_start..sym_end];
                    let at_rel = block.find("(at ").ok_or_else(|| {
                        anyhow::anyhow!("component '{reference}' has no position")
                    })?;
                    let value_start = sym_start + at_rel + "(at ".len();
                    let value_end = sym_start
                        + at_rel
                        + block[at_rel..].find(')').ok_or_else(|| {
                            anyhow::anyhow!("component '{reference}' has an invalid position")
                        })?;
                    let parts: Vec<&str> =
                        content[value_start..value_end].split_whitespace().collect();
                    let x: f64 = parts
                        .first()
                        .ok_or_else(|| anyhow::anyhow!("missing x"))?
                        .parse()?;
                    let y: f64 = parts
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("missing y"))?
                        .parse()?;
                    let rotation: f64 = parts.get(2).unwrap_or(&"0").parse()?;
                    let (new_x, new_y) = snap_point(x + dx, y + dy, 1.27);
                    edits.push(SexpEdit::replace(
                        value_start,
                        value_end,
                        format!("{new_x} {new_y} {rotation}"),
                    ));
                    changes.push(json!({
                        "kind": "component_moved",
                        "reference": reference,
                        "before": { "x": x, "y": y, "rotation": rotation },
                        "after": { "x": new_x, "y": new_y, "rotation": rotation }
                    }));
                }
            }
            WorkflowOperation::TransformFootprint { .. } => {
                anyhow::bail!("PCB operation is not supported by the schematic transformer")
            }
        }
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    for pair in edits.windows(2) {
        if pair[1].start < pair[0].end {
            anyhow::bail!(
                "operations overlap at schematic byte ranges {}..{} and {}..{}; combine each target field into one edit",
                pair[0].start,
                pair[0].end,
                pair[1].start,
                pair[1].end
            );
        }
    }
    Ok(TransformResult {
        content: apply_edits(content.to_string(), edits),
        changes,
    })
}

fn find_symbol_block(content: &str, reference: &str) -> Option<(usize, usize)> {
    let pattern = format!(r#"(property "Reference" "{reference}""#);
    let reference_offset = content.find(&pattern)?;
    let before = &content[..reference_offset];
    let start = ["\n  (symbol", "\n\t(symbol"]
        .iter()
        .filter_map(|pattern| before.rfind(pattern))
        .max()
        .map(|position| position + 1)?;
    find_block_with_leading_whitespace(content, start)
}

fn field_value_range(content: &str, reference: &str, field: &str) -> Option<(usize, usize)> {
    let (start, end) = find_symbol_block(content, reference)?;
    let block = &content[start..end];
    let pattern = format!(r#"(property "{field}" ""#);
    let relative = block.find(&pattern)?;
    let value_start = start + relative + pattern.len();
    let value_end = find_unescaped_quote(content, value_start, end)?;
    Some((value_start, value_end))
}

fn find_unescaped_quote(content: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut escaped = false;
    for (offset, byte) in bytes[start..end].iter().enumerate() {
        if *byte == b'"' && !escaped {
            return Some(start + offset);
        }
        escaped = *byte == b'\\' && !escaped;
        if *byte != b'\\' {
            escaped = false;
        }
    }
    None
}

fn symbol_scalar_range(content: &str, reference: &str, field: &str) -> Option<(usize, usize)> {
    let (start, end) = find_symbol_block(content, reference)?;
    let block = &content[start..end];
    let pattern = format!("({field} ");
    let relative = block.find(&pattern)?;
    let value_start = start + relative + pattern.len();
    let value_end = value_start
        + content[value_start..end]
            .find(|character: char| character.is_whitespace() || character == ')')?;
    Some((value_start, value_end))
}

fn escape_quoted_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn validate_reference(reference: &str) -> anyhow::Result<()> {
    if reference.is_empty() || reference.contains(['"', '\n', '\r']) {
        anyhow::bail!("invalid component reference '{reference}'");
    }
    Ok(())
}

fn allow_list(operations: &[WorkflowOperation]) -> Vec<String> {
    let mut targets = Vec::new();
    for operation in operations {
        match operation {
            WorkflowOperation::EditComponents { edits } => {
                targets.extend(edits.iter().map(|edit| edit.reference.clone()));
            }
            WorkflowOperation::MoveComponents { references, .. } => {
                targets.extend(references.clone())
            }
            WorkflowOperation::TransformFootprint { reference, .. } => {
                targets.push(reference.clone())
            }
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn canonical_resource(args: &Value, key: &str, extension: &str) -> anyhow::Result<PathBuf> {
    let raw = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing '{key}'"))?;
    let path = std::fs::canonicalize(raw)?;
    if !path.is_file() {
        anyhow::bail!("resource is not a regular file: {}", path.display());
    }
    if path.to_str().is_none() {
        anyhow::bail!("workflow resources must have a Unicode path");
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        anyhow::bail!("expected a .{extension} file: {}", path.display());
    }
    Ok(path)
}

fn required_id(args: &Value) -> anyhow::Result<&str> {
    args.get("change_set_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing change_set_id"))
}

fn sha256(bytes: &[u8]) -> String {
    // FIPS 180-4 SHA-256. Kept local so the workflow safety boundary does not
    // add a network-fetched dependency to otherwise offline builds.
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sigma1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sigma0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (target, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *target = target.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

fn hash_file(path: &Path) -> anyhow::Result<String> {
    Ok(sha256(&std::fs::read(path)?))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_schematic_text(content: &str, original_path: &Path) -> anyhow::Result<()> {
    let _ = original_path;
    let temp = tempfile::Builder::new().suffix(".kicad_sch").tempfile()?;
    std::fs::write(temp.path(), content)?;
    konnect_schematic_editor::Schematic::load(temp.path())?;
    Ok(())
}

fn inspect_file(path: &PathBuf) -> Value {
    match std::fs::metadata(path) {
        Ok(metadata) => json!({
            "path": path.display().to_string(),
            "exists": true,
            "bytes": metadata.len(),
            "sha256": hash_file(path).ok()
        }),
        Err(_) => json!({ "path": path.display().to_string(), "exists": false }),
    }
}

fn resolve_design_paths(
    args: &Value,
) -> anyhow::Result<(Option<PathBuf>, Option<PathBuf>, Option<PathBuf>)> {
    let mut project = None;
    let mut schematic = args
        .get("schematic")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let mut board = args.get("board").and_then(Value::as_str).map(PathBuf::from);
    if let Some(raw) = args.get("path").and_then(Value::as_str) {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            let mut projects: Vec<PathBuf> = std::fs::read_dir(&path)?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|entry| {
                    entry
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("kicad_pro"))
                })
                .collect();
            projects.sort();
            if projects.len() > 1 {
                anyhow::bail!(
                    "design directory is ambiguous; found multiple projects: {}",
                    projects
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            project = projects.into_iter().next();
        } else {
            match path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
            {
                "kicad_pro" => project = Some(path),
                "kicad_sch" => schematic = Some(path),
                "kicad_pcb" => board = Some(path),
                _ => anyhow::bail!("unsupported design path: {}", path.display()),
            }
        }
    }
    if let Some(project_path) = &project {
        let stem = project_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let parent = project_path.parent().unwrap_or_else(|| Path::new("."));
        schematic.get_or_insert_with(|| parent.join(format!("{stem}.kicad_sch")));
        board.get_or_insert_with(|| parent.join(format!("{stem}.kicad_pcb")));
    }
    if project.is_none() && schematic.is_none() && board.is_none() {
        anyhow::bail!("provide path, schematic, or board");
    }
    Ok((project, schematic, board))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::ToolRouter;

    const SCHEMATIC: &str = r#"(kicad_sch
  (version 20231120)
  (generator eeschema)
  (uuid "root")
  (paper "A4")
  (symbol
    (lib_id "Device:R")
    (at 10.16 20.32 0)
    (unit 1)
    (in_bom yes)
    (on_board yes)
    (dnp no)
    (uuid "r1")
    (property "Reference" "R1" (at 10.16 17.78 0))
    (property "Value" "1k" (at 10.16 20.32 0))
    (property "Footprint" "" (at 10.16 20.32 0) hide)
  )
)"#;

    fn edit_operation() -> WorkflowOperation {
        WorkflowOperation::EditComponents {
            edits: vec![ComponentEdit {
                reference: "R1".into(),
                value: Some("4.7k".into()),
                footprint: None,
                in_bom: None,
                dnp: Some(true),
                fields: HashMap::new(),
            }],
        }
    }

    #[test]
    fn transform_is_pure_and_machine_readable() {
        let result = transform_schematic(SCHEMATIC, &[edit_operation()]).unwrap();
        assert!(result.content.contains("\"Value\" \"4.7k\""));
        assert!(result.content.contains("(dnp yes)"));
        assert_eq!(result.changes[0]["reference"], "R1");
        assert!(SCHEMATIC.contains("\"Value\" \"1k\""));
    }

    #[test]
    fn move_is_grid_snapped() {
        let operation = WorkflowOperation::MoveComponents {
            references: vec!["R1".into()],
            dx: 1.27,
            dy: -1.27,
        };
        let result = transform_schematic(SCHEMATIC, &[operation]).unwrap();
        assert!(result.content.contains("(at 11.43 19.05 0)"));
    }

    #[test]
    fn missing_target_rejects_whole_transform() {
        let operation = WorkflowOperation::MoveComponents {
            references: vec!["R1".into(), "R404".into()],
            dx: 1.27,
            dy: 0.0,
        };
        assert!(transform_schematic(SCHEMATIC, &[operation]).is_err());
    }

    #[test]
    fn overlapping_component_edits_are_rejected() {
        let first = edit_operation();
        let mut second = edit_operation();
        let WorkflowOperation::EditComponents { edits } = &mut second else {
            unreachable!()
        };
        edits[0].value = Some("22k".into());
        assert!(transform_schematic(SCHEMATIC, &[first, second]).is_err());

        let moves = [
            WorkflowOperation::MoveComponents {
                references: vec!["R1".into()],
                dx: 1.27,
                dy: 0.0,
            },
            WorkflowOperation::MoveComponents {
                references: vec!["R1".into()],
                dx: 0.0,
                dy: 1.27,
            },
        ];
        assert!(transform_schematic(SCHEMATIC, &moves).is_err());
    }

    #[test]
    fn generic_fields_cannot_override_typed_properties() {
        let mut fields = HashMap::new();
        fields.insert("Reference".into(), "R2".into());
        let operation = WorkflowOperation::EditComponents {
            edits: vec![ComponentEdit {
                reference: "R1".into(),
                value: None,
                footprint: None,
                in_bom: None,
                dnp: None,
                fields,
            }],
        };
        assert!(transform_schematic(SCHEMATIC, &[operation]).is_err());
    }

    #[test]
    fn projected_courtyard_uses_absolute_position_and_rotation() {
        let snapshot = LivePcbSnapshot {
            contents: String::new(),
            footprints: vec![IpcFootprint {
                reference: "U1".into(),
                value: String::new(),
                footprint: String::new(),
                position: IpcVector2 { x: 10.0, y: 20.0 },
                definition_anchor: IpcVector2 { x: 10.0, y: 20.0 },
                definition_item_samples: Vec::new(),
                definition_item_types: Vec::new(),
                rotation: 0.0,
                layer: "F.Cu".into(),
                exclude_from_bom: false,
                dnp: false,
            }],
            courtyards: vec![IpcFootprintCourtyard {
                reference: "U1".into(),
                layer: "F.CrtYd".into(),
                bounds: None,
                primitives: vec![konnect_ipc::types::IpcCourtyardPrimitive {
                    kind: "segment".into(),
                    layer: "F.CrtYd".into(),
                    points: vec![
                        IpcVector2 { x: 11.0, y: 20.0 },
                        IpcVector2 { x: 12.0, y: 20.0 },
                    ],
                }],
            }],
        };
        let projected = projected_courtyards(
            &snapshot,
            &[IpcFootprintTransform {
                reference: "U1".into(),
                position: Some(IpcVector2 { x: 30.0, y: 40.0 }),
                rotation: Some(90.0),
            }],
        )
        .unwrap();
        let points = &projected[0].primitives[0].points;
        assert!((points[0].x - 30.0).abs() < 1e-9);
        assert!((points[0].y - 41.0).abs() < 1e-9);
        assert!((points[1].x - 30.0).abs() < 1e-9);
        assert!((points[1].y - 42.0).abs() < 1e-9);
    }

    #[test]
    fn pcb_transform_contract_rejects_partial_and_duplicate_targets() {
        let partial = [WorkflowOperation::TransformFootprint {
            reference: "U1".into(),
            x: Some(10.0),
            y: None,
            rotation: None,
        }];
        assert!(ipc_transforms(&partial).is_err());

        let duplicate = [
            WorkflowOperation::TransformFootprint {
                reference: "U1".into(),
                x: Some(10.0),
                y: Some(20.0),
                rotation: None,
            },
            WorkflowOperation::TransformFootprint {
                reference: "U1".into(),
                x: None,
                y: None,
                rotation: Some(90.0),
            },
        ];
        assert!(ipc_transforms(&duplicate).is_err());
    }

    #[test]
    fn quoted_values_are_escaped() {
        assert_eq!(escape_quoted_value("a\\b\"c"), "a\\\\b\\\"c");
    }

    #[test]
    fn sha256_matches_fips_vector() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn workflow_tool_surface_is_small_and_stable() {
        let names: Vec<_> = tools().into_iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            [
                "inspect_design",
                "plan_schematic_edit",
                "plan_pcb_edit",
                "get_change_set",
                "apply_change_set",
                "verify_change_set",
                "discard_change_set"
            ]
        );
    }

    fn context() -> ToolContext {
        ToolContext::new(
            crate::tools::ServerConfig {
                kicad_cli: "kicad-cli".into(),
                kicad_binary: "kicad".into(),
                ipc_address: "ipc:///unused".into(),
                project_dir: None,
                jlcpcb_db_path: None,
            },
            Arc::new(ToolRouter::new()),
        )
    }

    fn result_json(result: CallToolResult) -> Value {
        match &result.content[0] {
            crate::mcp::protocol::ToolContent::Text { text } => serde_json::from_str(text).unwrap(),
            _ => panic!("expected JSON text result"),
        }
    }

    #[tokio::test]
    async fn plan_is_zero_write_then_apply_and_verify_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.kicad_sch");
        std::fs::write(&path, SCHEMATIC).unwrap();
        let args = json!({
            "schematic": path,
            "operations": [{
                "kind": "edit_components",
                "edits": [{ "reference": "R1", "value": "10k" }]
            }]
        });
        let ctx = context();
        let planned = result_json(handle_plan_schematic(&args, &ctx).await.unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SCHEMATIC);
        assert_eq!(planned["lifecycle"], "planned");
        let id = planned["id"].as_str().unwrap();

        let applied = result_json(
            handle_apply(&json!({ "change_set_id": id }), &ctx)
                .await
                .unwrap(),
        );
        assert_eq!(applied["lifecycle"], "applied");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"Value\" \"10k\""));
        let reapplied = result_json(
            handle_apply(&json!({ "change_set_id": id }), &ctx)
                .await
                .unwrap(),
        );
        assert_eq!(reapplied["lifecycle"], "applied");

        let verified = result_json(
            handle_verify(&json!({ "change_set_id": id }), &ctx)
                .await
                .unwrap(),
        );
        assert_eq!(verified["lifecycle"], "verified");
    }

    #[tokio::test]
    async fn stale_plan_is_rejected_without_overwriting_newer_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.kicad_sch");
        std::fs::write(&path, SCHEMATIC).unwrap();
        let ctx = context();
        let planned = result_json(
            handle_plan_schematic(
                &json!({
                    "schematic": path,
                    "operations": [{
                        "kind": "move_components",
                        "references": ["R1"], "dx": 1.27, "dy": 0
                    }]
                }),
                &ctx,
            )
            .await
            .unwrap(),
        );
        let id = planned["id"].as_str().unwrap();
        let newer = SCHEMATIC.replace("1k", "2.2k");
        std::fs::write(&path, &newer).unwrap();

        let stale = result_json(
            handle_apply(&json!({ "change_set_id": id }), &ctx)
                .await
                .unwrap(),
        );
        assert_eq!(stale["lifecycle"], "stale");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    }

    #[tokio::test]
    async fn concurrent_apply_retries_remain_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.kicad_sch");
        std::fs::write(&path, SCHEMATIC).unwrap();
        let ctx = context();
        let planned = result_json(
            handle_plan_schematic(
                &json!({
                    "schematic": path,
                    "operations": [{
                        "kind": "edit_components",
                        "edits": [{ "reference": "R1", "value": "33k" }]
                    }]
                }),
                &ctx,
            )
            .await
            .unwrap(),
        );
        let args = json!({ "change_set_id": planned["id"] });
        let (first, second) = tokio::join!(handle_apply(&args, &ctx), handle_apply(&args, &ctx));
        assert_eq!(result_json(first.unwrap())["lifecycle"], "applied");
        assert_eq!(result_json(second.unwrap())["lifecycle"], "applied");
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("\"Value\" \"33k\""));
    }

    #[tokio::test]
    async fn concurrent_apply_and_discard_have_one_legal_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.kicad_sch");
        std::fs::write(&path, SCHEMATIC).unwrap();
        let ctx = context();
        let planned = result_json(
            handle_plan_schematic(
                &json!({
                    "schematic": path,
                    "operations": [{
                        "kind": "edit_components",
                        "edits": [{ "reference": "R1", "value": "47k" }]
                    }]
                }),
                &ctx,
            )
            .await
            .unwrap(),
        );
        let id = planned["id"].as_str().unwrap();
        let args = json!({ "change_set_id": id });
        let _ = tokio::join!(handle_apply(&args, &ctx), handle_discard(&args, &ctx));
        let final_run = store().get(id).unwrap();
        match final_run.lifecycle {
            LifecycleState::Applied => assert!(std::fs::read_to_string(path)
                .unwrap()
                .contains("\"Value\" \"47k\"")),
            LifecycleState::Discarded => {
                assert_eq!(std::fs::read_to_string(path).unwrap(), SCHEMATIC)
            }
            state => panic!("illegal concurrent terminal state: {state:?}"),
        }
    }
}
