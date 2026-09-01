//! Process-local state for guarded design workflows.
//!
//! This module deliberately contains no MCP tool definitions. It provides the
//! serializable workflow contract and the atomic, per-`ToolContext` state that
//! later handlers can build on without falling back to process-global state.

use crate::gates::{combined_status, GateStatus};
use konnect_sexp::command::SchematicCommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

pub const WORKFLOW_TTL: Duration = Duration::from_secs(30 * 60);
pub const MAX_WORKFLOWS: usize = 256;

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
        x: Option<f64>,
        y: Option<f64>,
        rotation: Option<f64>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::ToolRouter;
    use crate::tools::{ServerConfig, ToolContext};
    use std::sync::Barrier;

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
