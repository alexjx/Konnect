//! Process-local state for guarded design workflows.
//!
//! This module deliberately contains no MCP tool definitions. It provides the
//! serializable workflow contract and the atomic, per-`ToolContext` state that
//! later handlers can build on without falling back to process-global state.

use crate::gates::{combined_status, GateStatus};
use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::sch_components::{
    plan_schematic_component_edits, plan_schematic_component_moves, SchematicComponentEdit,
};
use crate::tools::{
    invalid_arg, require_str, with_board_ipc_classified, BoardAccess, ToolContext, ToolDef,
};
use konnect_ipc::types::{IpcBounds, IpcFootprint, IpcFootprintCourtyard};
use konnect_sexp::command::SchematicCommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

pub const WORKFLOW_TTL: Duration = Duration::from_secs(30 * 60);
pub const MAX_WORKFLOWS: usize = 256;

/// Guarded read/plan tools. This collection is intentionally not wired into
/// `ALL_TOOLSETS` yet; registering it is a separate exposure decision.
pub fn tools() -> Vec<ToolDef> {
    vec![
        tool!(
            "inspect_design",
            "Inspect one KiCad design resource without changing it.",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string", "minLength": 1 } },
                "required": ["path"],
                "additionalProperties": false
            }),
            |args, ctx| async move { handle_inspect_design(args, ctx).await }
        ),
        tool!(
            "plan_schematic_edit",
            "Build and retain an immutable, zero-write schematic change set.",
            schematic_plan_schema(),
            |args, ctx| async move { handle_plan_schematic_edit(args, ctx).await }
        ),
        tool!(
            "plan_pcb_edit",
            "Preflight complete footprint placements against the exact live board.",
            pcb_plan_schema(),
            |args, ctx| async move { handle_plan_pcb_edit(args, ctx).await }
        )
        .with_board_access(BoardAccess::LiveOnly),
        tool!(
            "get_change_set",
            "Read one process-local guarded workflow change set.",
            change_set_schema(),
            |args, ctx| async move { handle_get_change_set(args, ctx).await }
        ),
        tool!(
            "discard_change_set",
            "Discard a planned change set that has produced no effects.",
            change_set_schema(),
            |args, ctx| async move { handle_discard_change_set(args, ctx).await }
        ),
    ]
}

fn change_set_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "change_set_id": { "type": "string", "minLength": 1 } },
        "required": ["change_set_id"],
        "additionalProperties": false
    })
}

fn schematic_plan_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "schematic": { "type": "string", "minLength": 1 },
            "operations": {
                "type": "array",
                "minItems": 1,
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
                                            "fields": { "type": "object", "additionalProperties": { "type": "string" } }
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
                }
            }
        },
        "required": ["schematic", "operations"],
        "additionalProperties": false
    })
}

fn pcb_plan_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "board": { "type": "string", "minLength": 1 },
            "operations": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "const": "transform_footprint" },
                        "reference": { "type": "string", "minLength": 1 },
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "rotation": { "type": "number" }
                    },
                    "required": ["kind", "reference"],
                    "anyOf": [{ "required": ["x", "y"] }, { "required": ["rotation"] }],
                    "additionalProperties": false
                }
            }
        },
        "required": ["board", "operations"],
        "additionalProperties": false
    })
}

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
    Verifying,
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

impl EffectState {
    fn must_retain(&self) -> bool {
        matches!(self, Self::LiveDocument | Self::Unknown)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRecord {
    pub name: String,
    pub status: GateStatus,
    /// Machine-readable observations used to reach the verdict.
    #[serde(default)]
    pub evidence: Value,
    /// Thresholds, skipped prerequisites, and other explanatory metadata.
    #[serde(default)]
    pub details: Value,
}

impl GateRecord {
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        matches!(
            self.status,
            GateStatus::Fail | GateStatus::Blocked | GateStatus::Empty
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
        x: f64,
        y: f64,
        rotation: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesignDiff {
    pub before_sha256: String,
    pub after_sha256: String,
    pub changes: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditPlan {
    pub resource: String,
    pub base_sha256: String,
    pub operations: Vec<WorkflowOperation>,
    pub allow_list: Vec<String>,
    pub expected_diff: Option<DesignDiff>,
    #[serde(default)]
    pub validation_baseline: Vec<GateRecord>,
}

impl EditPlan {
    #[must_use]
    pub fn combined_gate_status(&self) -> GateStatus {
        combined_status(self.validation_baseline.iter().map(|record| record.status))
    }

    pub fn blocking_gates(&self) -> impl Iterator<Item = &GateRecord> {
        self.validation_baseline
            .iter()
            .filter(|record| record.is_blocking())
    }

    #[must_use]
    pub fn gates_allow_apply(&self) -> bool {
        matches!(
            self.combined_gate_status(),
            GateStatus::Pass | GateStatus::Warn
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl WorkflowRun {
    pub fn planned(
        id: impl Into<String>,
        domain: WorkflowDomain,
        resource: impl Into<String>,
        base_sha256: impl Into<String>,
        operations: Vec<WorkflowOperation>,
    ) -> Self {
        let created_at_unix = unix_now();
        Self {
            id: id.into(),
            api_version: 1,
            domain,
            lifecycle: LifecycleState::Planned,
            effect_state: EffectState::None,
            plan: EditPlan {
                resource: resource.into(),
                base_sha256: base_sha256.into(),
                operations,
                allow_list: Vec::new(),
                expected_diff: None,
                validation_baseline: Vec::new(),
            },
            actual_diff: None,
            error: None,
            created_at_unix,
            expires_at_unix: created_at_unix + WORKFLOW_TTL.as_secs(),
        }
    }

    fn must_retain(&self) -> bool {
        self.effect_state.must_retain()
            || matches!(
                self.lifecycle,
                LifecycleState::Applying | LifecycleState::Verifying
            )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceInspection {
    pub path: String,
    pub exists: bool,
    pub sha256: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InspectionResult {
    pub source: String,
    pub project: Option<ResourceInspection>,
    pub schematic: Option<ResourceInspection>,
    pub board: Option<ResourceInspection>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDisposition {
    Started,
    Idempotent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowTransitionResult {
    pub disposition: TransitionDisposition,
    pub run: WorkflowRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApplyOutcome {
    Applied {
        effect_state: EffectState,
        actual_diff: DesignDiff,
    },
    Stale {
        error: WorkflowError,
    },
    Rejected {
        error: WorkflowError,
    },
    Failed {
        effect_state: EffectState,
        error: WorkflowError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VerificationOutcome {
    Verified {
        effect_state: EffectState,
    },
    Failed {
        effect_state: EffectState,
        error: WorkflowError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowStoreError {
    #[error("change set '{0}' was not found or expired")]
    NotFound(String),
    #[error("change set '{0}' already exists")]
    Duplicate(String),
    #[error("workflow store is full of live or outcome-unknown change sets")]
    CapacityExhausted,
    #[error("change set '{id}' cannot {action} from state {state:?}")]
    InvalidTransition {
        id: String,
        action: &'static str,
        state: LifecycleState,
    },
    #[error("cannot canonicalize workflow resource {path:?}: {source}")]
    InvalidResource {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
enum StoredCommand {
    Schematic(SchematicCommand),
}

#[derive(Debug, Clone)]
struct StoredWorkflow {
    run: WorkflowRun,
    command: Option<StoredCommand>,
}

#[derive(Default)]
pub struct WorkflowStore {
    runs: Mutex<HashMap<String, StoredWorkflow>>,
    locks: Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
}

impl WorkflowStore {
    pub fn insert(
        &self,
        run: WorkflowRun,
        schematic_command: Option<SchematicCommand>,
    ) -> Result<WorkflowRun, WorkflowStoreError> {
        self.insert_at(run, schematic_command, unix_now())
    }

    pub fn get(&self, id: &str) -> Option<WorkflowRun> {
        self.get_at(id, unix_now())
    }

    pub fn schematic_command(&self, id: &str) -> Option<SchematicCommand> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, unix_now());
        match runs.get(id)?.command.as_ref()? {
            StoredCommand::Schematic(command) => Some(command.clone()),
        }
    }

    pub fn resource_lock(
        &self,
        resource: &Path,
    ) -> Result<Arc<AsyncMutex<()>>, WorkflowStoreError> {
        let canonical = std::fs::canonicalize(resource).map_err(|source| {
            WorkflowStoreError::InvalidResource {
                path: resource.to_path_buf(),
                source,
            }
        })?;
        let mut locks = self.locks.lock().expect("workflow lock map poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&canonical).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(canonical, Arc::downgrade(&lock));
        Ok(lock)
    }

    pub fn begin_apply(&self, id: &str) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        self.transition(id, "apply", |run| match &run.lifecycle {
            LifecycleState::Planned => {
                Some((LifecycleState::Applying, TransitionDisposition::Started))
            }
            LifecycleState::Applying => {
                Some((LifecycleState::Applying, TransitionDisposition::Idempotent))
            }
            LifecycleState::Applied | LifecycleState::Verified => {
                Some((run.lifecycle.clone(), TransitionDisposition::Idempotent))
            }
            _ => None,
        })
    }

    pub fn finish_apply(
        &self,
        id: &str,
        outcome: ApplyOutcome,
    ) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, unix_now());
        let stored = runs
            .get_mut(id)
            .ok_or_else(|| WorkflowStoreError::NotFound(id.to_owned()))?;
        if stored.run.lifecycle != LifecycleState::Applying {
            if matches!(
                stored.run.lifecycle,
                LifecycleState::Applied | LifecycleState::Verified
            ) {
                return Ok(WorkflowTransitionResult {
                    disposition: TransitionDisposition::Idempotent,
                    run: stored.run.clone(),
                });
            }
            return Err(invalid_transition(id, "finish apply", &stored.run));
        }
        match outcome {
            ApplyOutcome::Applied {
                effect_state,
                actual_diff,
            } => {
                stored.run.lifecycle = LifecycleState::Applied;
                stored.run.effect_state = effect_state;
                stored.run.actual_diff = Some(actual_diff);
                stored.run.error = None;
            }
            ApplyOutcome::Stale { error } => {
                stored.run.lifecycle = LifecycleState::Stale;
                stored.run.error = Some(error);
            }
            ApplyOutcome::Rejected { error } => {
                stored.run.lifecycle = LifecycleState::Rejected;
                stored.run.error = Some(error);
            }
            ApplyOutcome::Failed {
                effect_state,
                error,
            } => {
                stored.run.lifecycle = LifecycleState::Failed;
                stored.run.effect_state = effect_state;
                stored.run.error = Some(error);
            }
        }
        Ok(started(stored.run.clone()))
    }

    pub fn begin_verify(&self, id: &str) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        self.transition(id, "verify", |run| match &run.lifecycle {
            LifecycleState::Applied => {
                Some((LifecycleState::Verifying, TransitionDisposition::Started))
            }
            LifecycleState::Verifying => {
                Some((LifecycleState::Verifying, TransitionDisposition::Idempotent))
            }
            LifecycleState::VerificationFailed if run.effect_state != EffectState::Unknown => {
                Some((LifecycleState::Verifying, TransitionDisposition::Started))
            }
            LifecycleState::Verified => {
                Some((LifecycleState::Verified, TransitionDisposition::Idempotent))
            }
            _ => None,
        })
    }

    pub fn finish_verify(
        &self,
        id: &str,
        outcome: VerificationOutcome,
    ) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, unix_now());
        let stored = runs
            .get_mut(id)
            .ok_or_else(|| WorkflowStoreError::NotFound(id.to_owned()))?;
        if stored.run.lifecycle != LifecycleState::Verifying {
            if stored.run.lifecycle == LifecycleState::Verified {
                return Ok(WorkflowTransitionResult {
                    disposition: TransitionDisposition::Idempotent,
                    run: stored.run.clone(),
                });
            }
            return Err(invalid_transition(id, "finish verify", &stored.run));
        }
        match outcome {
            VerificationOutcome::Verified { effect_state } => {
                stored.run.lifecycle = LifecycleState::Verified;
                stored.run.effect_state = effect_state;
                stored.run.error = None;
            }
            VerificationOutcome::Failed {
                effect_state,
                error,
            } => {
                stored.run.lifecycle = LifecycleState::VerificationFailed;
                stored.run.effect_state = effect_state;
                stored.run.error = Some(error);
            }
        }
        Ok(started(stored.run.clone()))
    }

    pub fn discard(&self, id: &str) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        self.transition(id, "discard", |run| match &run.lifecycle {
            LifecycleState::Planned if run.effect_state == EffectState::None => {
                Some((LifecycleState::Discarded, TransitionDisposition::Started))
            }
            LifecycleState::Discarded => {
                Some((LifecycleState::Discarded, TransitionDisposition::Idempotent))
            }
            _ => None,
        })
    }

    pub fn len(&self) -> usize {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, unix_now());
        runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn insert_at(
        &self,
        run: WorkflowRun,
        schematic_command: Option<SchematicCommand>,
        now: u64,
    ) -> Result<WorkflowRun, WorkflowStoreError> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, now);
        if runs.contains_key(&run.id) {
            return Err(WorkflowStoreError::Duplicate(run.id));
        }
        if runs.len() >= MAX_WORKFLOWS {
            let evictable = runs
                .values()
                .filter(|stored| !stored.run.must_retain())
                .min_by(|left, right| {
                    (left.run.created_at_unix, left.run.id.as_str())
                        .cmp(&(right.run.created_at_unix, right.run.id.as_str()))
                })
                .map(|stored| stored.run.id.clone())
                .ok_or(WorkflowStoreError::CapacityExhausted)?;
            runs.remove(&evictable);
        }
        let result = run.clone();
        runs.insert(
            run.id.clone(),
            StoredWorkflow {
                run,
                command: schematic_command.map(StoredCommand::Schematic),
            },
        );
        Ok(result)
    }

    fn get_at(&self, id: &str, now: u64) -> Option<WorkflowRun> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, now);
        runs.get(id).map(|stored| stored.run.clone())
    }

    fn transition(
        &self,
        id: &str,
        action: &'static str,
        decide: impl FnOnce(&WorkflowRun) -> Option<(LifecycleState, TransitionDisposition)>,
    ) -> Result<WorkflowTransitionResult, WorkflowStoreError> {
        let mut runs = self.runs.lock().expect("workflow store poisoned");
        Self::purge_expired(&mut runs, unix_now());
        let stored = runs
            .get_mut(id)
            .ok_or_else(|| WorkflowStoreError::NotFound(id.to_owned()))?;
        let (next, disposition) =
            decide(&stored.run).ok_or_else(|| invalid_transition(id, action, &stored.run))?;
        stored.run.lifecycle = next;
        if stored.run.lifecycle == LifecycleState::Discarded {
            stored.run.error = None;
            stored.command = None;
        }
        Ok(WorkflowTransitionResult {
            disposition,
            run: stored.run.clone(),
        })
    }

    fn purge_expired(runs: &mut HashMap<String, StoredWorkflow>, now: u64) {
        runs.retain(|_, stored| stored.run.expires_at_unix > now || stored.run.must_retain());
    }
}

fn invalid_transition(id: &str, action: &'static str, run: &WorkflowRun) -> WorkflowStoreError {
    WorkflowStoreError::InvalidTransition {
        id: id.to_owned(),
        action,
        state: run.lifecycle.clone(),
    }
}

fn started(run: WorkflowRun) -> WorkflowTransitionResult {
    WorkflowTransitionResult {
        disposition: TransitionDisposition::Started,
        run,
    }
}

#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn handle_inspect_design(args: &Value, _ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let raw = match require_str(args, "path") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let path = std::fs::canonicalize(raw)?;
    if !path.is_file() {
        return Ok(CallToolResult::error(format!(
            "design resource is not a regular file: {}",
            path.display()
        )));
    }
    let bytes = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&bytes);
    let extension = path.extension().and_then(|value| value.to_str());
    let mut details = BTreeMap::from([("bytes".to_string(), serde_json::json!(bytes.len()))]);
    if matches!(extension, Some("kicad_sch" | "kicad_pcb")) {
        let parsed = konnect_sexp::parser::parse_sexp(&text)?;
        details.insert(
            "root".into(),
            serde_json::json!(parsed.head().unwrap_or("<empty>")),
        );
    } else if extension == Some("kicad_pro") {
        serde_json::from_slice::<Value>(&bytes)?;
        details.insert("root".into(), serde_json::json!("kicad_project"));
    }
    let resource = ResourceInspection {
        path: path.display().to_string(),
        exists: true,
        sha256: Some(sha256(&bytes)),
        details,
    };
    let result = InspectionResult {
        source: "file".into(),
        project: (extension == Some("kicad_pro")).then(|| resource.clone()),
        schematic: (extension == Some("kicad_sch")).then(|| resource.clone()),
        board: (extension == Some("kicad_pcb")).then_some(resource),
    };
    Ok(CallToolResult::json(&result))
}

async fn handle_plan_schematic_edit(
    args: &Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let raw = match require_str(args, "schematic") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let operations: Vec<WorkflowOperation> = match args.get("operations") {
        Some(value) => match serde_json::from_value::<Vec<WorkflowOperation>>(value.clone()) {
            Ok(operations) if !operations.is_empty() => operations,
            Ok(_) => return Ok(invalid_arg("operations", "must not be empty")),
            Err(error) => return Ok(invalid_arg("operations", &error.to_string())),
        },
        None => return Ok(invalid_arg("operations", "missing or not an array")),
    };
    if operations
        .iter()
        .any(|operation| matches!(operation, WorkflowOperation::TransformFootprint { .. }))
    {
        return Ok(invalid_arg(
            "operations",
            "PCB transforms are not valid in a schematic plan",
        ));
    }
    let resource = canonical_resource(raw, "kicad_sch")?;
    let source = std::fs::read_to_string(&resource)?;
    let mut run = WorkflowRun::planned(
        uuid::Uuid::new_v4().to_string(),
        WorkflowDomain::Schematic,
        resource.display().to_string(),
        sha256(source.as_bytes()),
        operations.clone(),
    );
    run.plan.validation_baseline.push(pass_gate(
        "resource",
        serde_json::json!({ "path": resource }),
    ));
    let parse_result = konnect_sexp::parser::parse_sexp(&source)
        .and_then(|root| {
            if root.head() == Some("kicad_sch") {
                Ok(root)
            } else {
                Err(konnect_sexp::SexpError::InvalidValue(
                    "document root is not kicad_sch".into(),
                ))
            }
        })
        .map(|_| ());
    let mut candidate = source.clone();
    let mut changes = Vec::new();
    let mut planner_error = parse_result.err().map(|error| error.to_string());
    if planner_error.is_none() {
        for operation in operations.iter().cloned() {
            let planned = match operation {
                WorkflowOperation::EditComponents { edits } => {
                    let edits = edits
                        .iter()
                        .map(|edit| SchematicComponentEdit {
                            reference: edit.reference.clone(),
                            new_reference: None,
                            value: edit.value.clone(),
                            footprint: edit.footprint.clone(),
                            datasheet: None,
                            in_bom: edit.in_bom,
                            dnp: edit.dnp,
                            fields: edit.fields.clone(),
                        })
                        .collect::<Vec<_>>();
                    plan_schematic_component_edits(&candidate, &edits).map(|plan| {
                        let operation_changes = plan
                            .changes
                            .into_iter()
                            .map(|change| serde_json::json!({ "kind": "component_edit", "summary": change }));
                        (plan.candidate, operation_changes.collect::<Vec<_>>())
                    })
                }
                WorkflowOperation::MoveComponents { references, dx, dy } => {
                    plan_schematic_component_moves(&candidate, &references, dx, dy).map(|plan| {
                        let operation_changes = plan
                            .placements
                            .into_iter()
                            .map(|placement| {
                                serde_json::json!({
                                    "kind": "component_moved",
                                    "reference": placement.reference,
                                    "unit": placement.unit,
                                    "before": { "x": placement.old_x, "y": placement.old_y },
                                    "after": { "x": placement.new_x, "y": placement.new_y }
                                })
                            })
                            .collect();
                        (plan.candidate, operation_changes)
                    })
                }
                WorkflowOperation::TransformFootprint { .. } => unreachable!(),
            };
            match planned {
                Ok((next, operation_changes)) => {
                    candidate = next;
                    changes.extend(operation_changes);
                }
                Err(error) => {
                    planner_error = Some(error.to_string());
                    break;
                }
            }
        }
    }
    let command = if let Some(error) = planner_error {
        run.plan.validation_baseline.push(GateRecord {
            name: "schematic_plan".into(),
            status: GateStatus::Fail,
            evidence: Value::Null,
            details: serde_json::json!({ "error": error }),
        });
        None
    } else {
        run.plan.validation_baseline.push(pass_gate(
            "schematic_parse",
            serde_json::json!({ "root": "kicad_sch" }),
        ));
        run.plan.validation_baseline.push(pass_gate(
            "schematic_plan",
            serde_json::json!({ "changes": changes.len() }),
        ));
        run.plan.expected_diff = Some(DesignDiff {
            before_sha256: sha256(source.as_bytes()),
            after_sha256: sha256(candidate.as_bytes()),
            changes,
        });
        Some(
            SchematicCommand::from_document_diff(&source, &candidate, "Guarded schematic edit")?
                .requiring_unchanged_document(),
        )
    };
    run.plan.allow_list = operation_references(&operations).into_iter().collect();
    let stored = ctx.workflow_store.insert(run, command)?;
    Ok(CallToolResult::json(&stored))
}

#[derive(Debug, Clone, Deserialize)]
struct PcbTransformRequest {
    kind: String,
    reference: String,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    rotation: Option<f64>,
}

#[derive(Debug, Clone)]
struct LivePcbSnapshot {
    source: String,
    footprints: Vec<IpcFootprint>,
    courtyards: Vec<IpcFootprintCourtyard>,
}

async fn handle_plan_pcb_edit(args: &Value, ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let raw = match require_str(args, "board") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let requests: Vec<PcbTransformRequest> = match args.get("operations") {
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(requests) => requests,
            Err(error) => return Ok(invalid_arg("operations", &error.to_string())),
        },
        None => return Ok(invalid_arg("operations", "missing or not an array")),
    };
    let board = canonical_resource(raw, "kicad_pcb")?;
    let requested = board.clone();
    let snapshot = match with_board_ipc_classified(ctx, &board, move |client| {
        let document = client.find_open_board(&requested)?;
        Ok(LivePcbSnapshot {
            source: client.save_document_to_string_in(document.clone())?,
            footprints: client.list_footprints_in(document.clone())?,
            courtyards: client.list_footprint_courtyards_in(document)?,
        })
    })
    .await?
    {
        Ok(snapshot) => snapshot,
        Err(error) => return Ok(CallToolResult::error(error.to_string())),
    };
    let run = plan_pcb_snapshot(&board, snapshot, &requests);
    let stored = ctx.workflow_store.insert(run, None)?;
    Ok(CallToolResult::json(&stored))
}

async fn handle_get_change_set(args: &Value, ctx: &ToolContext) -> anyhow::Result<CallToolResult> {
    let id = match require_str(args, "change_set_id") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    Ok(match ctx.workflow_store.get(id) {
        Some(run) => CallToolResult::json(&run),
        None => CallToolResult::error(format!("change set '{id}' was not found or expired")),
    })
}

async fn handle_discard_change_set(
    args: &Value,
    ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let id = match require_str(args, "change_set_id") {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    Ok(match ctx.workflow_store.discard(id) {
        Ok(result) => CallToolResult::json(&result.run),
        Err(error) => CallToolResult::error(error.to_string()),
    })
}

fn canonical_resource(raw: &str, extension: &str) -> anyhow::Result<PathBuf> {
    let path = std::fs::canonicalize(raw)?;
    if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some(extension) {
        anyhow::bail!(
            "workflow resource must be an existing .{extension} file: {}",
            path.display()
        );
    }
    if path.to_str().is_none() {
        anyhow::bail!("workflow resource paths must be Unicode");
    }
    Ok(path)
}

fn operation_references(operations: &[WorkflowOperation]) -> BTreeSet<String> {
    let mut references = BTreeSet::new();
    for operation in operations {
        match operation {
            WorkflowOperation::EditComponents { edits } => {
                references.extend(edits.iter().map(|edit| edit.reference.clone()));
            }
            WorkflowOperation::MoveComponents {
                references: moved, ..
            } => references.extend(moved.iter().cloned()),
            WorkflowOperation::TransformFootprint { reference, .. } => {
                references.insert(reference.clone());
            }
        }
    }
    references
}

fn pass_gate(name: &str, evidence: Value) -> GateRecord {
    GateRecord {
        name: name.into(),
        status: GateStatus::Pass,
        evidence,
        details: Value::Null,
    }
}

fn plan_pcb_snapshot(
    resource: &Path,
    snapshot: LivePcbSnapshot,
    requests: &[PcbTransformRequest],
) -> WorkflowRun {
    let base_sha256 = sha256(snapshot.source.as_bytes());
    let mut run = WorkflowRun::planned(
        uuid::Uuid::new_v4().to_string(),
        WorkflowDomain::Pcb,
        resource.display().to_string(),
        base_sha256.clone(),
        Vec::new(),
    );
    run.plan.validation_baseline.push(pass_gate(
        "resource",
        serde_json::json!({ "path": resource }),
    ));
    let parse_status = konnect_sexp::parser::parse_sexp(&snapshot.source)
        .map(|root| root.head() == Some("kicad_pcb"))
        .unwrap_or(false);
    run.plan.validation_baseline.push(GateRecord {
        name: "parse".into(),
        status: if parse_status {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        evidence: serde_json::json!({ "root": "kicad_pcb" }),
        details: if parse_status {
            Value::Null
        } else {
            serde_json::json!({ "error": "live snapshot is not a parseable kicad_pcb document" })
        },
    });
    run.plan.validation_baseline.push(pass_gate(
        "exact_board_identity",
        serde_json::json!({ "path": resource }),
    ));
    run.plan.validation_baseline.push(GateRecord {
        name: "board_population".into(),
        status: if snapshot.footprints.is_empty() {
            GateStatus::Empty
        } else {
            GateStatus::Pass
        },
        evidence: serde_json::json!({ "footprints": snapshot.footprints.len() }),
        details: Value::Null,
    });

    let mut footprint_counts = HashMap::<&str, usize>::new();
    let mut footprints = HashMap::<&str, &IpcFootprint>::new();
    for footprint in &snapshot.footprints {
        *footprint_counts.entry(&footprint.reference).or_default() += 1;
        footprints.entry(&footprint.reference).or_insert(footprint);
    }
    let duplicate_live = footprint_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(reference, _)| (*reference).to_string())
        .collect::<BTreeSet<_>>();
    let mut seen_requests = HashSet::new();
    let mut request_issues = Vec::new();
    let mut hydrated = Vec::new();
    for request in requests {
        if request.kind != "transform_footprint" {
            request_issues.push(format!("unsupported PCB operation '{}'", request.kind));
            continue;
        }
        if request.reference.trim().is_empty() {
            request_issues.push("target reference is empty".to_string());
            continue;
        }
        if !seen_requests.insert(request.reference.as_str()) {
            request_issues.push(format!(
                "target '{}' appears more than once",
                request.reference
            ));
            continue;
        }
        if request.x.is_some() != request.y.is_some() {
            request_issues.push(format!(
                "target '{}' must provide x and y together",
                request.reference
            ));
            continue;
        }
        if request.x.is_none() && request.rotation.is_none() {
            request_issues.push(format!(
                "target '{}' contains no transform",
                request.reference
            ));
            continue;
        }
        let Some(current) = footprints.get(request.reference.as_str()) else {
            request_issues.push(format!(
                "target '{}' is absent from the live board",
                request.reference
            ));
            continue;
        };
        let x = request.x.unwrap_or(current.position.x);
        let y = request.y.unwrap_or(current.position.y);
        let rotation = request.rotation.unwrap_or(current.rotation);
        if !x.is_finite() || !y.is_finite() || !rotation.is_finite() {
            request_issues.push(format!(
                "target '{}' contains a non-finite placement",
                request.reference
            ));
            continue;
        }
        hydrated.push(WorkflowOperation::TransformFootprint {
            reference: request.reference.clone(),
            x,
            y,
            rotation,
        });
    }
    if !duplicate_live.is_empty() {
        request_issues.push(format!(
            "live board repeats references: {}",
            duplicate_live.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let uniqueness_status = if requests.is_empty() {
        GateStatus::Empty
    } else if request_issues.is_empty() {
        GateStatus::Pass
    } else {
        GateStatus::Fail
    };
    run.plan.validation_baseline.push(GateRecord {
        name: "target_uniqueness".into(),
        status: uniqueness_status,
        evidence: serde_json::json!({
            "requested": requests.len(),
            "hydrated": hydrated.len(),
            "issues": request_issues
        }),
        details: Value::Null,
    });

    let target_references = hydrated
        .iter()
        .filter_map(|operation| match operation {
            WorkflowOperation::TransformFootprint { reference, .. } => Some(reference.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let usable_references = snapshot
        .courtyards
        .iter()
        .filter(|courtyard| courtyard_bounds(courtyard).is_some())
        .map(|courtyard| courtyard.reference.as_str())
        .collect::<HashSet<_>>();
    let missing_courtyards = target_references
        .iter()
        .filter(|reference| !usable_references.contains(reference.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    run.plan.validation_baseline.push(GateRecord {
        name: "usable_courtyard".into(),
        status: if target_references.is_empty() {
            GateStatus::Empty
        } else if missing_courtyards.is_empty() {
            GateStatus::Pass
        } else {
            GateStatus::Blocked
        },
        evidence: serde_json::json!({ "missing": missing_courtyards }),
        details: Value::Null,
    });

    let baseline_boxes = snapshot
        .courtyards
        .iter()
        .filter_map(|courtyard| {
            courtyard_bounds(courtyard).map(|bounds| CourtyardBox {
                reference: courtyard.reference.clone(),
                layer: courtyard.layer.clone(),
                bounds,
            })
        })
        .collect::<Vec<_>>();
    let baseline_overlaps = overlap_pairs(&baseline_boxes);
    run.plan.validation_baseline.push(GateRecord {
        name: "baseline_overlap".into(),
        status: if baseline_overlaps.is_empty() {
            GateStatus::Pass
        } else {
            GateStatus::Warn
        },
        evidence: serde_json::json!({ "pairs": &baseline_overlaps }),
        details: serde_json::json!({ "semantics": "pre-existing overlaps are retained as warnings" }),
    });

    let placements = hydrated
        .iter()
        .filter_map(|operation| match operation {
            WorkflowOperation::TransformFootprint {
                reference,
                x,
                y,
                rotation,
            } => Some((reference.as_str(), (*x, *y, *rotation))),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let projected_boxes = snapshot
        .courtyards
        .iter()
        .filter_map(|courtyard| {
            let current = footprints.get(courtyard.reference.as_str())?;
            let placement = placements
                .get(courtyard.reference.as_str())
                .copied()
                .unwrap_or((current.position.x, current.position.y, current.rotation));
            projected_courtyard_bounds(courtyard, current, placement).map(|bounds| CourtyardBox {
                reference: courtyard.reference.clone(),
                layer: courtyard.layer.clone(),
                bounds,
            })
        })
        .collect::<Vec<_>>();
    let projected_overlaps = overlap_pairs(&projected_boxes);
    let new_overlaps = projected_overlaps
        .difference(&baseline_overlaps)
        .cloned()
        .collect::<BTreeSet<_>>();
    run.plan.validation_baseline.push(GateRecord {
        name: "new_overlap".into(),
        status: if new_overlaps.is_empty() {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        evidence: serde_json::json!({ "pairs": new_overlaps }),
        details: Value::Null,
    });
    run.plan.validation_baseline.push(pass_gate(
        "revision",
        serde_json::json!({ "sha256": base_sha256 }),
    ));
    run.plan.operations = hydrated;
    run.plan.allow_list = target_references.into_iter().collect();
    run
}

#[derive(Debug, Clone)]
struct CourtyardBox {
    reference: String,
    layer: String,
    bounds: (f64, f64, f64, f64),
}

fn courtyard_bounds(courtyard: &IpcFootprintCourtyard) -> Option<(f64, f64, f64, f64)> {
    let IpcBounds { min, max } = courtyard.bounds.as_ref()?;
    let values = [min.x, min.y, max.x, max.y];
    (values.iter().all(|value| value.is_finite()) && min.x < max.x && min.y < max.y)
        .then_some((min.x, min.y, max.x, max.y))
}

fn projected_courtyard_bounds(
    courtyard: &IpcFootprintCourtyard,
    current: &IpcFootprint,
    placement: (f64, f64, f64),
) -> Option<(f64, f64, f64, f64)> {
    let points = courtyard
        .primitives
        .iter()
        .flat_map(|primitive| primitive.points.iter().map(|point| (point.x, point.y)))
        .collect::<Vec<_>>();
    let points = if points.is_empty() {
        let (min_x, min_y, max_x, max_y) = courtyard_bounds(courtyard)?;
        vec![
            (min_x, min_y),
            (min_x, max_y),
            (max_x, min_y),
            (max_x, max_y),
        ]
    } else {
        points
    };
    let radians = (placement.2 - current.rotation).to_radians();
    let (sin, cos) = radians.sin_cos();
    let transformed = points.into_iter().map(|(x, y)| {
        let local_x = x - current.position.x;
        let local_y = y - current.position.y;
        (
            placement.0 + local_x * cos - local_y * sin,
            placement.1 + local_x * sin + local_y * cos,
        )
    });
    bounds_of_points(transformed)
}

fn bounds_of_points(points: impl IntoIterator<Item = (f64, f64)>) -> Option<(f64, f64, f64, f64)> {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for (x, y) in points {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((min_x, min_y, max_x, max_y)) => {
                (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
            }
        });
    }
    bounds.filter(|(min_x, min_y, max_x, max_y)| min_x < max_x && min_y < max_y)
}

fn overlap_pairs(boxes: &[CourtyardBox]) -> BTreeSet<String> {
    let mut overlaps = BTreeSet::new();
    for (index, left) in boxes.iter().enumerate() {
        for right in &boxes[index + 1..] {
            if left.reference == right.reference || left.layer != right.layer {
                continue;
            }
            let intersects = left.bounds.0 < right.bounds.2
                && right.bounds.0 < left.bounds.2
                && left.bounds.1 < right.bounds.3
                && right.bounds.1 < left.bounds.3;
            if intersects {
                let mut references = [left.reference.as_str(), right.reference.as_str()];
                references.sort_unstable();
                overlaps.insert(format!(
                    "{}:{}:{}",
                    left.layer, references[0], references[1]
                ));
            }
        }
    }
    overlaps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::ToolContent;
    use crate::router::ToolRouter;
    use crate::tools::{ServerConfig, ToolContext};
    use konnect_ipc::gen::kiapi;
    use prost::Message;
    use std::sync::Barrier;

    const SCHEMATIC: &str = r#"(kicad_sch
  (version 20260306)
  (uuid "11111111-1111-4111-8111-111111111111")
  (symbol
    (lib_id "Device:R")
    (at 100 100 0)
    (unit 1)
    (in_bom yes)
    (dnp no)
    (uuid "22222222-2222-4222-8222-222222222222")
    (property "Reference" "R1" (at 100 98 0))
    (property "Value" "1k" (at 100 102 0))
    (property "Footprint" "" (at 100 100 0))
    (property "Datasheet" "" (at 100 100 0))
    (instances
      (project "design"
        (path "/11111111-1111-4111-8111-111111111111"
          (reference "R1")
          (unit 1)
        )
      )
    )
  )
  (sheet_instances (path "/" (page "1")))
)
"#;

    fn run(id: impl Into<String>, created_at_unix: u64, effect_state: EffectState) -> WorkflowRun {
        let id = id.into();
        WorkflowRun {
            id,
            api_version: 1,
            domain: WorkflowDomain::Schematic,
            lifecycle: LifecycleState::Planned,
            effect_state,
            plan: EditPlan {
                resource: "design.kicad_sch".into(),
                base_sha256: sha256(b"before"),
                operations: Vec::new(),
                allow_list: Vec::new(),
                expected_diff: None,
                validation_baseline: Vec::new(),
            },
            actual_diff: None,
            error: None,
            created_at_unix,
            expires_at_unix: created_at_unix + WORKFLOW_TTL.as_secs(),
        }
    }

    fn context() -> ToolContext {
        ToolContext::new(ServerConfig::default(), Arc::new(ToolRouter::new()))
    }

    fn context_with_ipc(ipc_address: String) -> ToolContext {
        ToolContext::new(
            ServerConfig {
                ipc_address,
                ..ServerConfig::default()
            },
            Arc::new(ToolRouter::new()),
        )
    }

    fn body(result: &CallToolResult) -> Value {
        let ToolContent::Text { text } = &result.content[0] else {
            panic!("expected text result")
        };
        serde_json::from_str(text).unwrap()
    }

    fn footprint(reference: &str, x: f64, y: f64) -> IpcFootprint {
        IpcFootprint {
            reference: reference.into(),
            value: String::new(),
            footprint: String::new(),
            position: konnect_ipc::types::IpcVector2 { x, y },
            rotation: 0.0,
            layer: "F.Cu".into(),
        }
    }

    fn courtyard(
        reference: &str,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> IpcFootprintCourtyard {
        IpcFootprintCourtyard {
            reference: reference.into(),
            layer: "F.CrtYd".into(),
            bounds: Some(IpcBounds {
                min: konnect_ipc::types::IpcVector2 { x: min_x, y: min_y },
                max: konnect_ipc::types::IpcVector2 { x: max_x, y: max_y },
            }),
            primitives: Vec::new(),
        }
    }

    fn gate<'a>(run: &'a WorkflowRun, name: &str) -> &'a GateRecord {
        run.plan
            .validation_baseline
            .iter()
            .find(|gate| gate.name == name)
            .unwrap()
    }

    #[test]
    fn workflow_stores_are_isolated_per_tool_context() {
        let first = context();
        let second = context();
        first
            .workflow_store
            .insert(run("one", unix_now(), EffectState::None), None)
            .unwrap();

        assert!(first.workflow_store.get("one").is_some());
        assert!(second.workflow_store.get("one").is_none());
        assert!(!Arc::ptr_eq(&first.workflow_store, &second.workflow_store));
    }

    #[test]
    fn unregistered_tool_defs_pin_required_fields_and_board_access() {
        let definitions = tools();
        let access = definitions
            .iter()
            .map(|definition| (definition.name, definition.board_access))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(access["inspect_design"], BoardAccess::None);
        assert_eq!(access["plan_schematic_edit"], BoardAccess::None);
        assert_eq!(access["plan_pcb_edit"], BoardAccess::LiveOnly);
        assert_eq!(access["get_change_set"], BoardAccess::None);
        assert_eq!(access["discard_change_set"], BoardAccess::None);
        for definition in definitions {
            assert!(definition.input_schema["required"]
                .as_array()
                .is_some_and(|required| !required.is_empty()));
        }
    }

    #[tokio::test]
    async fn handlers_return_structured_errors_for_missing_required_fields() {
        let context = Arc::new(context());
        for definition in tools() {
            let result = (definition.handler)(&serde_json::json!({}), Arc::clone(&context))
                .await
                .unwrap();
            assert!(
                result.is_error,
                "{} accepted missing fields",
                definition.name
            );
        }
    }

    #[tokio::test]
    async fn schematic_plan_is_zero_write_and_keeps_its_command_out_of_json() {
        let directory = tempfile::tempdir().unwrap();
        let schematic = directory.path().join("design.kicad_sch");
        std::fs::write(&schematic, SCHEMATIC).unwrap();
        let context = Arc::new(context());
        let before = std::fs::read(&schematic).unwrap();
        let result = handle_plan_schematic_edit(
            &serde_json::json!({
                "schematic": schematic,
                "operations": [{
                    "kind": "edit_components",
                    "edits": [{ "reference": "R1", "value": "47k", "dnp": true }]
                }]
            }),
            &context,
        )
        .await
        .unwrap();
        let value = body(&result);
        let run: WorkflowRun = serde_json::from_value(value.clone()).unwrap();

        assert_eq!(std::fs::read(&schematic).unwrap(), before);
        assert!(run.plan.gates_allow_apply());
        assert!(context.workflow_store.schematic_command(&run.id).is_some());
        assert!(value.get("command").is_none());
    }

    #[tokio::test]
    async fn pcb_plan_refuses_a_different_open_board_before_snapshot_reads() {
        use nng::options::Options;

        let directory = tempfile::tempdir().unwrap();
        let requested = directory.path().join("requested.kicad_pcb");
        let other = directory.path().join("other.kicad_pcb");
        std::fs::write(&requested, "(kicad_pcb)\n").unwrap();
        std::fs::write(&other, "(kicad_pcb)\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("tcp://127.0.0.1:{port}");
        let socket = nng::Socket::new(nng::Protocol::Rep0).unwrap();
        socket
            .set_opt::<nng::options::RecvTimeout>(Some(Duration::from_secs(10)))
            .unwrap();
        socket.listen(&url).unwrap();
        let other_name = other.display().to_string();
        let server = std::thread::spawn(move || {
            let message = socket.recv().unwrap();
            let request = kiapi::common::ApiRequest::decode(message.as_slice()).unwrap();
            assert!(request
                .message
                .is_some_and(|message| message.type_url.ends_with("GetOpenDocuments")));
            let document = kiapi::common::types::DocumentSpecifier {
                r#type: kiapi::common::types::DocumentType::DoctypePcb as i32,
                project: None,
                identifier: Some(
                    kiapi::common::types::document_specifier::Identifier::BoardFilename(other_name),
                ),
            };
            let response = kiapi::common::ApiResponse {
                status: Some(kiapi::common::ApiResponseStatus {
                    status: kiapi::common::ApiStatusCode::AsOk as i32,
                    error_message: String::new(),
                }),
                header: None,
                message: Some(konnect_ipc::builders::pack_any(
                    &kiapi::common::commands::GetOpenDocumentsResponse {
                        documents: vec![document],
                    },
                    "kiapi.common.commands.GetOpenDocumentsResponse",
                )),
            };
            socket
                .send(nng::Message::from(response.encode_to_vec().as_slice()))
                .unwrap();
        });
        let context = context_with_ipc(url);
        let result = handle_plan_pcb_edit(
            &serde_json::json!({
                "board": requested,
                "operations": [{
                    "kind": "transform_footprint",
                    "reference": "U1",
                    "x": 10.0,
                    "y": 10.0
                }]
            }),
            &context,
        )
        .await
        .unwrap();
        server.join().unwrap();

        assert!(result.is_error);
        assert!(context.workflow_store.is_empty());
        let ToolContent::Text { text } = &result.content[0] else {
            panic!("expected text error")
        };
        assert!(text.contains("not open in KiCAD"));
    }

    #[test]
    fn pcb_plan_hydrates_partial_placements_and_blocks_missing_courtyards() {
        let directory = tempfile::tempdir().unwrap();
        let board = directory.path().join("board.kicad_pcb");
        std::fs::write(&board, "(kicad_pcb)\n").unwrap();
        let run = plan_pcb_snapshot(
            &board,
            LivePcbSnapshot {
                source: "(kicad_pcb)\n".into(),
                footprints: vec![footprint("U1", 2.0, 3.0)],
                courtyards: Vec::new(),
            },
            &[PcbTransformRequest {
                kind: "transform_footprint".into(),
                reference: "U1".into(),
                x: Some(10.0),
                y: Some(11.0),
                rotation: None,
            }],
        );

        assert_eq!(gate(&run, "usable_courtyard").status, GateStatus::Blocked);
        assert!(!run.plan.gates_allow_apply());
        assert_eq!(
            run.plan.operations,
            [WorkflowOperation::TransformFootprint {
                reference: "U1".into(),
                x: 10.0,
                y: 11.0,
                rotation: 0.0,
            }]
        );
    }

    #[test]
    fn pcb_overlap_gates_warn_for_baseline_and_fail_only_for_new_pairs() {
        let directory = tempfile::tempdir().unwrap();
        let board = directory.path().join("board.kicad_pcb");
        std::fs::write(&board, "(kicad_pcb)\n").unwrap();
        let baseline = plan_pcb_snapshot(
            &board,
            LivePcbSnapshot {
                source: "(kicad_pcb)\n".into(),
                footprints: vec![footprint("U1", 0.0, 0.0), footprint("U2", 1.0, 0.0)],
                courtyards: vec![
                    courtyard("U1", -1.0, -1.0, 1.0, 1.0),
                    courtyard("U2", 0.0, -1.0, 2.0, 1.0),
                ],
            },
            &[PcbTransformRequest {
                kind: "transform_footprint".into(),
                reference: "U1".into(),
                x: None,
                y: None,
                rotation: Some(0.0),
            }],
        );
        assert_eq!(gate(&baseline, "baseline_overlap").status, GateStatus::Warn);
        assert_eq!(gate(&baseline, "new_overlap").status, GateStatus::Pass);
        assert!(baseline.plan.gates_allow_apply());

        let introduced = plan_pcb_snapshot(
            &board,
            LivePcbSnapshot {
                source: "(kicad_pcb)\n".into(),
                footprints: vec![footprint("U1", 0.0, 0.0), footprint("U2", 5.0, 0.0)],
                courtyards: vec![
                    courtyard("U1", -1.0, -1.0, 1.0, 1.0),
                    courtyard("U2", 4.0, -1.0, 6.0, 1.0),
                ],
            },
            &[PcbTransformRequest {
                kind: "transform_footprint".into(),
                reference: "U2".into(),
                x: Some(1.0),
                y: Some(0.0),
                rotation: None,
            }],
        );
        assert_eq!(
            gate(&introduced, "baseline_overlap").status,
            GateStatus::Pass
        );
        assert_eq!(gate(&introduced, "new_overlap").status, GateStatus::Fail);
        assert!(!introduced.plan.gates_allow_apply());
    }

    #[tokio::test]
    async fn discard_handler_only_discards_an_effect_free_plan_and_is_idempotent() {
        let context = context();
        context
            .workflow_store
            .insert(run("discard", unix_now(), EffectState::None), None)
            .unwrap();
        for _ in 0..2 {
            let result = handle_discard_change_set(
                &serde_json::json!({ "change_set_id": "discard" }),
                &context,
            )
            .await
            .unwrap();
            assert_eq!(body(&result)["lifecycle"], "discarded");
        }
        let mut applied = run("applied", unix_now(), EffectState::PersistedToDisk);
        applied.lifecycle = LifecycleState::Applied;
        context.workflow_store.insert(applied, None).unwrap();
        let refused =
            handle_discard_change_set(&serde_json::json!({ "change_set_id": "applied" }), &context)
                .await
                .unwrap();
        assert!(refused.is_error);
    }

    #[test]
    fn canonical_equivalent_paths_share_one_resource_lock() {
        let directory = tempfile::tempdir().unwrap();
        let resource = directory.path().join("design.kicad_sch");
        std::fs::write(&resource, "(kicad_sch)\n").unwrap();
        let equivalent = directory.path().join(".").join("design.kicad_sch");
        let store = WorkflowStore::default();

        let first = store.resource_lock(&resource).unwrap();
        let second = store.resource_lock(&equivalent).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn ttl_never_expires_live_or_unknown_effects() {
        let store = WorkflowStore::default();
        let now = WORKFLOW_TTL.as_secs() + 10;
        store
            .insert_at(run("ordinary", 0, EffectState::None), None, 0)
            .unwrap();
        store
            .insert_at(run("live", 0, EffectState::LiveDocument), None, 0)
            .unwrap();
        store
            .insert_at(run("unknown", 0, EffectState::Unknown), None, 0)
            .unwrap();

        assert!(store.get_at("ordinary", now).is_none());
        assert!(store.get_at("live", now).is_some());
        assert!(store.get_at("unknown", now).is_some());
    }

    #[test]
    fn ttl_never_expires_an_apply_or_verify_in_progress() {
        let store = WorkflowStore::default();
        let now = WORKFLOW_TTL.as_secs() + 10;
        let mut applying = run("applying", 0, EffectState::None);
        applying.lifecycle = LifecycleState::Applying;
        let mut verifying = run("verifying", 0, EffectState::None);
        verifying.lifecycle = LifecycleState::Verifying;
        store.insert_at(applying, None, 0).unwrap();
        store.insert_at(verifying, None, 0).unwrap();

        assert!(store.get_at("applying", now).is_some());
        assert!(store.get_at("verifying", now).is_some());
    }

    #[test]
    fn capacity_evicts_only_safe_runs_and_refuses_to_drop_protected_runs() {
        let store = WorkflowStore::default();
        for index in 0..MAX_WORKFLOWS {
            store
                .insert_at(
                    run(format!("protected-{index:03}"), 0, EffectState::Unknown),
                    None,
                    0,
                )
                .unwrap();
        }
        assert!(matches!(
            store.insert_at(run("overflow", 1, EffectState::None), None, 1),
            Err(WorkflowStoreError::CapacityExhausted)
        ));
        assert_eq!(store.runs.lock().unwrap().len(), MAX_WORKFLOWS);

        let safe = WorkflowStore::default();
        for index in 0..MAX_WORKFLOWS {
            safe.insert_at(
                run(format!("safe-{index:03}"), index as u64, EffectState::None),
                None,
                index as u64,
            )
            .unwrap();
        }
        safe.insert_at(
            run("replacement", MAX_WORKFLOWS as u64, EffectState::None),
            None,
            MAX_WORKFLOWS as u64,
        )
        .unwrap();
        assert!(safe.get_at("safe-000", MAX_WORKFLOWS as u64).is_none());
        assert!(safe.get_at("replacement", MAX_WORKFLOWS as u64).is_some());
        assert_eq!(safe.runs.lock().unwrap().len(), MAX_WORKFLOWS);
    }

    #[test]
    fn capacity_never_evicts_an_apply_or_verify_in_progress() {
        let store = WorkflowStore::default();
        let mut applying = run("applying", 0, EffectState::None);
        applying.lifecycle = LifecycleState::Applying;
        let mut verifying = run("verifying", 1, EffectState::None);
        verifying.lifecycle = LifecycleState::Verifying;
        store.insert_at(applying, None, 0).unwrap();
        store.insert_at(verifying, None, 1).unwrap();
        for index in 2..MAX_WORKFLOWS {
            store
                .insert_at(
                    run(format!("safe-{index:03}"), index as u64, EffectState::None),
                    None,
                    index as u64,
                )
                .unwrap();
        }

        store
            .insert_at(
                run("replacement", MAX_WORKFLOWS as u64, EffectState::None),
                None,
                MAX_WORKFLOWS as u64,
            )
            .unwrap();
        assert!(store.get_at("applying", MAX_WORKFLOWS as u64).is_some());
        assert!(store.get_at("verifying", MAX_WORKFLOWS as u64).is_some());
        assert!(store.get_at("safe-002", MAX_WORKFLOWS as u64).is_none());
    }

    #[test]
    fn gate_baselines_compose_and_only_failed_unrunnable_or_empty_gates_block() {
        let mut plan = run("gates", unix_now(), EffectState::None).plan;
        assert_eq!(plan.combined_gate_status(), GateStatus::Empty);
        assert!(!plan.gates_allow_apply());
        plan.validation_baseline = vec![
            GateRecord {
                name: "erc".into(),
                status: GateStatus::Pass,
                evidence: serde_json::json!({ "violations": 0 }),
                details: Value::Null,
            },
            GateRecord {
                name: "clearance".into(),
                status: GateStatus::Warn,
                evidence: serde_json::json!({ "minimum_mm": 0.19 }),
                details: serde_json::json!({ "pass_mm": 0.2 }),
            },
        ];
        assert_eq!(plan.combined_gate_status(), GateStatus::Warn);
        assert_eq!(plan.blocking_gates().count(), 0);
        assert!(plan.gates_allow_apply());

        plan.validation_baseline.push(GateRecord {
            name: "courtyard".into(),
            status: GateStatus::Fail,
            evidence: serde_json::json!({ "overlaps": ["U1/U2"] }),
            details: Value::Null,
        });
        assert_eq!(plan.combined_gate_status(), GateStatus::Fail);
        assert!(!plan.gates_allow_apply());
        assert_eq!(
            plan.blocking_gates()
                .map(|gate| gate.name.as_str())
                .collect::<Vec<_>>(),
            ["courtyard"]
        );

        plan.validation_baseline.push(GateRecord {
            name: "drc".into(),
            status: GateStatus::Blocked,
            evidence: Value::Null,
            details: serde_json::json!({ "reason": "kicad-cli unavailable" }),
        });
        assert_eq!(plan.combined_gate_status(), GateStatus::Blocked);
        assert!(!plan.gates_allow_apply());
    }

    #[test]
    fn apply_and_discard_race_has_one_winner_and_retries_are_idempotent() {
        let store = Arc::new(WorkflowStore::default());
        store
            .insert(run("race", unix_now(), EffectState::None), None)
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let apply = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.begin_apply("race")
            })
        };
        let discard = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.discard("race")
            })
        };
        barrier.wait();
        let apply = apply.join().unwrap();
        let discard = discard.join().unwrap();
        assert_ne!(apply.is_ok(), discard.is_ok());

        let current = store.get("race").unwrap();
        match current.lifecycle {
            LifecycleState::Applying => {
                let applied = store
                    .finish_apply(
                        "race",
                        ApplyOutcome::Applied {
                            effect_state: EffectState::PersistedToDisk,
                            actual_diff: DesignDiff {
                                before_sha256: sha256(b"before"),
                                after_sha256: sha256(b"after"),
                                changes: Vec::new(),
                            },
                        },
                    )
                    .unwrap();
                assert_eq!(applied.run.lifecycle, LifecycleState::Applied);
                assert_eq!(
                    store.begin_apply("race").unwrap().disposition,
                    TransitionDisposition::Idempotent
                );
                assert!(store.discard("race").is_err());
            }
            LifecycleState::Discarded => {
                assert_eq!(
                    store.discard("race").unwrap().disposition,
                    TransitionDisposition::Idempotent
                );
                assert!(store.begin_apply("race").is_err());
            }
            state => panic!("illegal race outcome: {state:?}"),
        }
    }

    #[test]
    fn concurrent_begin_apply_has_one_executor_and_one_idempotent_observer() {
        let store = Arc::new(WorkflowStore::default());
        store
            .insert(run("apply", unix_now(), EffectState::None), None)
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let callers = (0..2)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.begin_apply("apply").unwrap().disposition
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let dispositions = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            dispositions
                .iter()
                .filter(|&&result| result == TransitionDisposition::Started)
                .count(),
            1
        );
        assert_eq!(
            dispositions
                .iter()
                .filter(|&&result| result == TransitionDisposition::Idempotent)
                .count(),
            1
        );
        assert_eq!(
            store.get("apply").unwrap().lifecycle,
            LifecycleState::Applying
        );
    }

    #[test]
    fn verify_transitions_are_atomic_and_unknown_effects_cannot_retry() {
        let store = WorkflowStore::default();
        let mut applied = run("verify", unix_now(), EffectState::LiveDocument);
        applied.lifecycle = LifecycleState::Applied;
        store.insert(applied, None).unwrap();

        assert_eq!(
            store.begin_verify("verify").unwrap().run.lifecycle,
            LifecycleState::Verifying
        );
        let retry = store.begin_verify("verify").unwrap();
        assert_eq!(retry.disposition, TransitionDisposition::Idempotent);
        assert_eq!(retry.run.lifecycle, LifecycleState::Verifying);
        store
            .finish_verify(
                "verify",
                VerificationOutcome::Failed {
                    effect_state: EffectState::Unknown,
                    error: WorkflowError {
                        code: "save_failed".into(),
                        message: "save outcome unknown".into(),
                        retryable: false,
                    },
                },
            )
            .unwrap();
        assert!(store.begin_verify("verify").is_err());
    }

    #[test]
    fn workflow_results_serialize_without_internal_commands() {
        let run = run("serial", unix_now(), EffectState::None);
        let serialized = serde_json::to_value(&run).unwrap();
        assert_eq!(serialized["lifecycle"], "planned");
        assert!(serialized.get("command").is_none());
        assert_eq!(sha256(b"abc").len(), 64);
    }
}
